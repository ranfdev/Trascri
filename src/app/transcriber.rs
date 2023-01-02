// The role of this module is to glue the audio_src and recognizer adapters, run
// a gst_pipeline in a separate thread and offer a simple interface to communicate with the thread.

use std::sync::{Arc, Mutex};
use std::thread;

use byte_slice_cast::*;
use gst::element_error;
use gst::prelude::*;
use postage::mpsc::{channel, Receiver, Sender};
use postage::prelude::*;

use crate::ports::Recognizer;

#[derive(Debug)]
pub enum Msg {
    PartialResult(String),
    Result(String),
    Stopped,
    Started,
}

#[derive(Debug, Clone)]
pub enum InMsg {
    Start(Sender<Msg>),
    Stop,
    SetElement(gst::Element),
    Reset,
}

fn build_pipeline(
    sample_rate: i32,
    src: &gst::Element,
) -> anyhow::Result<(gst::Pipeline, gst_app::AppSink)> {
    let pipeline = gst::Pipeline::new(None);

    let appsink = gst_app::AppSink::builder()
        .caps(
            &gst_audio::AudioCapsBuilder::new_interleaved()
                .rate(sample_rate)
                .format(gst_audio::AUDIO_FORMAT_S16)
                .channels(1)
                .build(),
        )
        .build();
    pipeline.add(dbg!(src)).unwrap();
    pipeline.add(&appsink).unwrap();
    src.link(&appsink)?;
    Ok((pipeline, appsink))
}

fn handle_samples(
    appsink: &gst_app::AppSink,
    mut cb: impl FnMut(&[i16]) + std::marker::Send + 'static,
) -> anyhow::Result<()> {
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            // Add a handler to the "new-sample" signal.
            .new_sample(move |appsink| {
                // Pull the sample in question out of the appsink's buffer.
                let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or_else(|| {
                    element_error!(
                        appsink,
                        gst::ResourceError::Failed,
                        ("Failed to get buffer from appsink")
                    );

                    gst::FlowError::Error
                })?;

                // At this point, buffer is only a reference to an existing memory region somewhere.
                // When we want to access its content, we have to map it while requesting the required
                // mode of access (read, read/write).
                // This type of abstraction is necessary, because the buffer in question might not be
                // on the machine's main memory itself, but rather in the GPU's memory.
                // So mapping the buffer makes the underlying memory region accessible to us.
                // See: https://gstreamer.freedesktop.org/documentation/plugin-development/advanced/allocation.html
                let map = buffer.map_readable().map_err(|_| {
                    element_error!(
                        appsink,
                        gst::ResourceError::Failed,
                        ("Failed to map buffer readable")
                    );

                    gst::FlowError::Error
                })?;

                // We know what format the data in the memory region has, since we requested
                // it by setting the appsink's caps. So what we do here is interpret the
                // memory region we mapped as an array of signed 16 bit integers.
                let samples: &[i16] = map.as_slice_of::<i16>().map_err(|_| {
                    element_error!(
                        appsink,
                        gst::ResourceError::Failed,
                        ("Failed to interprete buffer as S16 PCM")
                    );

                    gst::FlowError::Error
                })?;
                cb(samples);

                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    Ok(())
}

pub struct TranscriberActor {
    pub sender: Sender<InMsg>,
}

impl TranscriberActor {
    pub fn new(
        init_recognizer: impl Fn() -> Box<dyn Recognizer<Sample = i16> + Send> + Send + 'static,
        rms_out: Sender<f64>,
    ) -> Self {
        let (sender, receiver) = channel(8);

        thread::spawn(move || {
            let mut ts = Transcriber::new(receiver, init_recognizer, rms_out);
            ts.start_msg_loop();
        });
        Self { sender }
    }
    pub fn start(&self, update_sender: Sender<Msg>) {
        self.sender
            .clone()
            .blocking_send(InMsg::Start(update_sender))
            .unwrap();
    }
    pub fn stop(&self) {
        self.sender.clone().blocking_send(InMsg::Stop).unwrap();
    }
    pub fn set_element(&self, el: gst::Element) {
        self.sender
            .clone()
            .blocking_send(InMsg::SetElement(el))
            .unwrap();
    }
}

pub struct Transcriber {
    element: gst::Element,
    recognizer: Arc<Mutex<Box<dyn Recognizer<Sample = i16> + Send>>>,
    pipeline: gst::Pipeline,
    receiver: Receiver<InMsg>,
    results_out: Option<Sender<Msg>>,
    rms_out: Sender<f64>,
}

impl Transcriber {
    pub fn new(
        receiver: Receiver<InMsg>,
        init_recognizer: impl Fn() -> Box<dyn Recognizer<Sample = i16> + Send> + Send + 'static,
        rms_out: Sender<f64>,
    ) -> Self {
        Self {
            element: gst::ElementFactory::make_with_name("pulsesrc", None).unwrap(),
            pipeline: gst::Pipeline::default(),
            recognizer: Arc::new(Mutex::new(init_recognizer())),
            receiver,
            results_out: None,
            rms_out,
        }
    }
    fn handle(&mut self, msg: InMsg) {
        dbg!(&msg);
        match msg {
            InMsg::SetElement(el) => {
                self.element = el;
            }
            InMsg::Start(chan) => {
                self.stop();
                self.results_out = Some(chan.clone());
                self.rebuild_pipeline();
                self.start_pipeline_loop().unwrap();
            }
            InMsg::Stop => self.stop(),
            InMsg::Reset => {
                self.recognizer.lock().unwrap().reset();
            }
        }
    }
    fn stop(&mut self) {
        self.pipeline.set_state(gst::State::Null).unwrap();
        self.pipeline.remove(&self.element).unwrap();
        self.recognizer.lock().unwrap().reset();
        self.results_out
            .take()
            .map(|mut x| x.blocking_send(Msg::Stopped));
    }
    fn start_msg_loop(&mut self) {
        dbg!("Msg loop started");
        while let Some(msg) = self.receiver.blocking_recv() {
            self.handle(msg);
        }
        dbg!("Transcriber msg loop ended");
    }

    fn start_pipeline_loop(&mut self) -> anyhow::Result<()> {
        if self.pipeline.state(None).1 == gst::State::Playing {
            return Err(anyhow::anyhow!("Pipeline is already running!"));
        }
        self.pipeline.set_state(gst::State::Playing)?;

        let bus = self
            .pipeline
            .bus()
            .expect("Pipeline without bus. Shouldn't happen!");

        let pipeline = self.pipeline.clone();

        self.results_out
            .as_mut()
            .unwrap()
            .blocking_send(Msg::Started)
            .unwrap();
        bus.connect_message(None, move |_, msg| {
            use gst::MessageView;

            match dbg!(msg.view()) {
                MessageView::Eos(..) => {
                    dbg!("Received Eos");
                    pipeline.set_state(gst::State::Null).unwrap();
                }
                MessageView::Error(err) => {
                    dbg!(err);
                    pipeline.set_state(gst::State::Null).unwrap();
                }
                _ => (),
            }
        });

        Ok(())
    }

    fn rebuild_pipeline(&mut self) {
        let (pipeline, sink) = build_pipeline(
            self.recognizer.lock().unwrap().sample_rate() as i32,
            &self.element,
        )
        .expect("Failed to build pipeline");
        self.pipeline = pipeline;

        const CHUNK_SIZE: usize = 1024 * 2;

        let mut results_out = self.results_out.as_mut().unwrap().clone();
        let mut rms_out = self.rms_out.clone();
        let mut buf = Vec::from([0i16; CHUNK_SIZE]);
        let rec = self.recognizer.clone();
        handle_samples(&sink, move |samples| {
            buf.extend_from_slice(samples);
            if buf.len() >= CHUNK_SIZE {
                let sum: f64 = buf
                    .iter()
                    .map(|sample| {
                        let f = f64::from(*sample) / f64::from(i16::MAX);
                        f * f
                    })
                    .sum();
                let rms = (sum / (samples.len() as f64)).sqrt();
                rms_out.blocking_send(rms).unwrap();

                let mut recognizer = rec.lock().unwrap();
                let dec_state = recognizer.feed(&buf);
                if dec_state == crate::ports::recognizer::DecodingState::Finalized {
                    let res = recognizer.result().unwrap();
                    let s = res
                        .words
                        .into_iter()
                        .map(|w| w.text)
                        .collect::<Vec<&str>>()
                        .join(" ");
                    results_out.blocking_send(Msg::Result(s)).unwrap();
                } else {
                    let res = recognizer.partial_result().unwrap();
                    let s = res
                        .words
                        .into_iter()
                        .map(|w| w.text)
                        .collect::<Vec<&str>>()
                        .join(" ");
                    results_out.blocking_send(Msg::PartialResult(s)).unwrap();
                }
                buf.truncate(0);
            }
        })
        .unwrap();
    }
}
