use std::path;

use anyhow::Context;

use crate::ports::recognizer::*;

pub struct Vosk(vosk::Recognizer, f32);

impl Vosk {
    pub fn new(model_path: &path::Path, sample_rate: f32) -> Self {
        let model = vosk::Model::new(model_path.to_str().unwrap()).unwrap();
        let mut recognizer = vosk::Recognizer::new(&model, sample_rate).unwrap();
        recognizer.set_max_alternatives(0);
        recognizer.set_words(true);
        recognizer.set_partial_words(true);
        Self(recognizer, sample_rate)
    }
}

impl Recognizer for Vosk {
    type Sample = i16;
    fn feed(&mut self, data: &[Self::Sample]) -> DecodingState {
        match self.0.accept_waveform(data) {
            vosk::DecodingState::Finalized => DecodingState::Finalized,
            vosk::DecodingState::Running => DecodingState::Running,
            vosk::DecodingState::Failed => panic!("Decoding failed"),
        }
    }
    fn result(&mut self) -> anyhow::Result<Recognized> {
        let r = self.0.result();
        let r = r
            .single()
            .context("extracting final vosk recognizer result")?;
        let words = r.result.into_iter().map(|w| Word {
            confidence: w.conf,
            text: w.word,
        });
        Ok(Recognized {
            words: words.collect(),
        })
    }
    fn partial_result(&mut self) -> anyhow::Result<Recognized> {
        let r = self.0.partial_result();
        let words = r.partial_result.into_iter().map(|w| Word {
            confidence: w.conf,
            text: w.word,
        });
        Ok(Recognized {
            words: words.collect(),
        })
    }
    fn sample_rate(&self) -> f32 {
        self.1
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}
