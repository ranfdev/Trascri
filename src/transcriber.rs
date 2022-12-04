use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use byte_slice_cast::*;
use gst::element_error;
use gst::prelude::*;
use gtk::glib;
use gtk::glib::MainContext;

use crate::ports::Recognizer;

pub enum Msg {
    PartialResult(String),
    Result(String),
    Stopped,
    Started,
}

fn build_pipeline(
    sample_rate: i32,
    src: &gst::Element,
) -> anyhow::Result<(gst::Pipeline, gst_app::AppSink)> {
    let pipeline = gst::Pipeline::new(Some("mic"));

    let appsink = gst_app::AppSink::builder()
        .caps(
            &gst_audio::AudioCapsBuilder::new_interleaved()
                .rate(sample_rate)
                .format(gst_audio::AUDIO_FORMAT_S16)
                .channels(1)
                .build(),
        )
        .build();
    pipeline.add(src).unwrap();
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

fn samples_to_transcriber_msg_channel(
    appsink: &gst_app::AppSink,
    recognizer: Arc<Mutex<Box<dyn Recognizer<Sample = i16> + Send>>>,
) -> glib::Receiver<Msg> {
    let (sender, receiver) = MainContext::channel(glib::PRIORITY_DEFAULT);
    handle_samples(appsink, move |samples| {
        let mut recognizer = recognizer.lock().unwrap();
        let dec_state = recognizer.feed(samples);
        if dec_state == crate::ports::recognizer::DecodingState::Finalized {
            let res = recognizer.result().unwrap();
            let s = res.words.into_iter().map(|w| w.text).collect::<String>();
            sender.send(Msg::Result(s)).unwrap();
        } else {
            let res = recognizer.partial_result().unwrap();
            let s = res.words.into_iter().map(|w| w.text).collect::<String>();
            sender.send(Msg::PartialResult(s)).unwrap();
        }
    })
    .unwrap();
    receiver
}
fn start_pipeline(pipeline: gst::Pipeline) -> anyhow::Result<()> {
    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline
        .bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                dbg!("Received Eos");
                break;
            }
            MessageView::Error(err) => {
                dbg!(err);
                pipeline.set_state(gst::State::Null)?;
            }
            _ => (),
        }
    }

    pipeline.set_state(gst::State::Null)?;
    dbg!("Pipeline thread finished");

    Ok(())
}

pub struct TranscriberActor {
    element: RefCell<gst::Element>,
    callback: Rc<dyn Fn(Msg)>,
    recognizer: Arc<Mutex<Box<dyn Recognizer<Sample = i16> + Send>>>,
    pipeline: RefCell<gst::Pipeline>,
    thread: RefCell<Option<thread::JoinHandle<()>>>,
}

impl TranscriberActor {
    pub fn new(
        recognizer: Box<dyn Recognizer<Sample = i16> + Send>,
        cb: impl Fn(Msg) + 'static,
    ) -> Self {
        let recognizer = Arc::new(Mutex::new(recognizer));

        let this = Self {
            element: RefCell::new(gst::ElementFactory::make_with_name("pulsesrc", None).unwrap()),
            callback: Rc::new(cb),
            pipeline: RefCell::new(gst::Pipeline::default()),
            recognizer,
            thread: RefCell::new(None),
        };
        this
    }
    pub fn start(&self) {
        self.stop();
        let pc = self.pipeline.borrow().clone();
        self.thread.replace(Some(thread::Builder::new()
            .spawn(|| {
                start_pipeline(pc).unwrap();
            })
            .unwrap()));
        dbg!("Started new transcriber thread");
        (self.callback)(Msg::Started);
    }
    pub fn stop(&self) {
        if let Some(bus) = self.pipeline.borrow().bus() {
            bus.post(gst::message::Eos::new()).unwrap_or_else(|_| {
                println!("Error sending stop message to bus");
            });
        }

        if let Some(j) = self.thread.take() {
            glib::MainContext::default().spawn(async move {
                glib::timeout_future(std::time::Duration::from_secs(1)).await;
                if !j.is_finished() {
                    panic!("Transcriber thread didn't stop after 1s'");
                }
            });
        }
        (self.callback)(Msg::Stopped);
    }
    pub fn reset(&self) {
        self.stop();
        self.recognizer.lock().unwrap().reset();
    }
    fn rebuild_pipeline(&self) {
        let (pipeline, sink) = build_pipeline(
            self.recognizer.lock().unwrap().sample_rate() as i32,
            &*self.element.borrow(),
        ).expect("Failed to build pipeline");
        self.pipeline.replace(pipeline);

        let cb = self.callback.clone();
        samples_to_transcriber_msg_channel(&sink, self.recognizer.clone())
            .attach(None, move |t| {
                cb(t);
                glib::Continue(true)
            });
    }
    pub fn set_element(&self, s: gst::Element) {
        self.element.replace(s);
        self.rebuild_pipeline();
    }
    pub fn state(&self) -> gst::State {
        self.pipeline.borrow().state(None).1
    }
}
