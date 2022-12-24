use gst::prelude::*;
use gtk::gio;
use gtk::prelude::*;

use crate::ports::audio_src::AudioSrc;

pub struct File(gio::File);

impl File {
    pub async fn get_file() -> gio::File {
        let dialog = gtk::FileChooserNative::new(
            Some("Select an audio file"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Open,
            None,
            None,
        );
        dialog.run_future().await;
        dialog.file().unwrap()
    }
}

impl From<gio::File> for File {
    fn from(value: gio::File) -> Self {
        Self(value)
    }
}

impl AudioSrc for File {
    fn make_element(&self) -> gst::Element {
        let element = gst::ElementFactory::make("filesrc").build().unwrap();
        element.set_property("location", &self.0.uri());
        element
    }
}
