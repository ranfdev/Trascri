pub struct Word<'a> {
    pub confidence: f32,
    pub text: &'a str,
}

pub struct Recognized<'a> {
    pub words: Vec<Word<'a>>,
}

#[derive(PartialEq, Eq)]
pub enum DecodingState {
    Finalized,
    Running,
}

pub trait Recognizer {
    type Sample;
    fn feed(&mut self, data: &[Self::Sample]) -> DecodingState;
    fn partial_result(&mut self) -> anyhow::Result<Recognized>;
    fn result(&mut self) -> anyhow::Result<Recognized>;
    fn sample_rate(&self) -> f32;
    fn reset(&mut self);
}
