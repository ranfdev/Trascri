use gst::prelude::*;
use gtk::gio;

use crate::ports::audio_src::AudioSrc;

pub struct Pulse {
    device: gst::Device,
}

impl Pulse {
    pub fn list_available() -> gio::ListStore {
        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some("Audio/Source"), None);
        monitor.set_show_all_devices(true);
        monitor.start().unwrap();
        let mut ls = gio::ListStore::new(gst::Device::static_type());
        ls.extend(monitor.devices().iter());
        monitor.stop();
        ls
    }
}

impl From<gst::Device> for Pulse {
    fn from(value: gst::Device) -> Self {
        Self { device: value }
    }
}

impl AudioSrc for Pulse {
    fn make_element(&self) -> gst::Element {
        self.device.create_element(None).unwrap()
    }
}
