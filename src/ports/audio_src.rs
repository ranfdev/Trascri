pub trait AudioSrc {
    fn make_element(&self) -> gst::Element;
}
