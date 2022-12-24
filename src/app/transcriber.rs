// The role of this module is to glue the audio_src and recognizer adapters, run
// a gst_pipeline in a separate thread and offer a simple interface to communicate with the thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use byte_slice_cast::*;
use gst::element_error;
use gst::prelude::*;
use gtk::glib;
use gtk::glib::MainContext;
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
    chan_in: Sender<InMsg>,
}

impl TranscriberActor {
    pub fn new(
        init_recognizer: impl Fn() -> Box<dyn Recognizer<Sample = i16> + Send> + Send + 'static,
        rms_out: Sender<f64>,
    ) -> Self {
        let chan_in = TranscriberThread::new(init_recognizer, rms_out);
        Self { chan_in }
    }
    pub fn send(&self, msg: InMsg) {
        self.chan_in.clone().blocking_send(msg);
    }
}

pub struct TranscriberThread {
    element: gst::Element,
    recognizer: Arc<Mutex<Box<dyn Recognizer<Sample = i16> + Send>>>,
    pipeline: gst::Pipeline,
    chan_in: Sender<InMsg>,
    receiver_in: Receiver<InMsg>,
    chan_out: Option<Sender<Msg>>,
    rms_out: Sender<f64>,
}

impl TranscriberThread {
    pub fn new(
        init_recognizer: impl Fn() -> Box<dyn Recognizer<Sample = i16> + Send> + Send + 'static,
        rms_out: Sender<f64>,
    ) -> Sender<InMsg> {
        let (sender_in, receiver_in) = channel(1);

        let sender_in_c = sender_in.clone();
        thread::spawn(move || {
            let mut this = Self {
                element: gst::ElementFactory::make_with_name("pulsesrc", None).unwrap(),
                pipeline: gst::Pipeline::default(),
                recognizer: Arc::new(Mutex::new(init_recognizer())),
                chan_in: sender_in_c,
                receiver_in,
                chan_out: None,
                rms_out,
            };
            this.start_msg_loop();
        });
        sender_in
    }
    fn handle(&mut self, msg: InMsg) {
        dbg!(&msg);
        match msg {
            InMsg::SetElement(el) => {
                self.element = el;
            }
            InMsg::Start(sender) => {
                self.stop();
                self.chan_out = Some(sender.clone());
                self.rebuild_pipeline();
                self.start_pipeline_loop();
            }
            InMsg::Stop => self.stop(),
            InMsg::Reset => {
                self.recognizer.lock().unwrap().reset();
            }
            _ => unreachable!("msg not handled in transcriber thread"),
        }
    }
    fn stop(&mut self) {
        self.pipeline.set_state(gst::State::Null).unwrap();
        self.pipeline.remove(&self.element);
        self.recognizer.lock().unwrap().reset();
        self.chan_out
            .take()
            .map(|mut x| x.blocking_send(Msg::Stopped));
    }
    fn start_msg_loop(&mut self) {
        dbg!("Msg loop started");
        while let Some(msg) = self.receiver_in.blocking_recv() {
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

        self.chan_out.as_mut().unwrap().blocking_send(Msg::Started);
        bus.connect_message(None, move |bus, msg| {
            use gst::MessageView;

            match dbg!(msg.view()) {
                MessageView::Eos(..) => {
                    dbg!("Received Eos");
                    pipeline.set_state(gst::State::Null);
                }
                MessageView::Error(err) => {
                    dbg!(err);
                    pipeline.set_state(gst::State::Null);
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

        let mut sender = self.chan_out.as_mut().unwrap().clone();
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
                rms_out.blocking_send(rms);

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
                    sender.blocking_send(Msg::Result(s)).unwrap();
                } else {
                    let res = recognizer.partial_result().unwrap();
                    let s = res
                        .words
                        .into_iter()
                        .map(|w| w.text)
                        .collect::<Vec<&str>>()
                        .join(" ");
                    sender.blocking_send(Msg::PartialResult(s)).unwrap();
                }
                buf.truncate(0);
            }
        });
    }
}
