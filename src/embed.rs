use candle_core::{DType, Device};
use fastembed::Qwen3TextEmbedding;

use crate::Error;

pub(crate) trait Embedder: Send {
    fn dim(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error>;
}

pub(crate) struct QwenEmbedder {
    model: Qwen3TextEmbedding,
    dim: usize,
}

impl QwenEmbedder {
    pub(crate) fn load(model_id: &str, dim: usize) -> Result<Self, Error> {
        let device = Device::Cpu;
        let model = Qwen3TextEmbedding::from_hf(model_id, &device, DType::F32, 8192)
            .map_err(|err| Error::Embed(err.to_string()))?;
        let width = model.config().hidden_size;
        if width != dim {
            return Err(Error::DimMismatch {
                expected: dim,
                found: width,
            });
        }
        Ok(Self { model, dim })
    }
}

impl Embedder for QwenEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        let vectors = self
            .model
            .embed(texts)
            .map_err(|err| Error::Embed(err.to_string()))?;
        for vector in &vectors {
            if vector.len() != self.dim || vector.iter().any(|value| !value.is_finite()) {
                return Err(Error::Embed("embedding is the wrong width".into()));
            }
        }
        Ok(vectors)
    }
}

pub(crate) struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub(crate) fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        Ok(texts
            .iter()
            .map(|text| hash_vector(text, self.dim))
            .collect())
    }
}

fn hash_vector(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dim];
    let mut slot = 0usize;
    for byte in text.bytes() {
        slot = slot.wrapping_mul(167).wrapping_add(byte as usize);
    }
    if dim > 0 {
        vector[slot % dim] = 1.0;
    }
    vector
}

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
pub(crate) struct MapEmbedder {
    dim: usize,
    vectors: HashMap<String, Vec<f32>>,
}

#[cfg(test)]
impl MapEmbedder {
    pub(crate) fn new(dim: usize, pairs: &[(&str, Vec<f32>)]) -> Self {
        let vectors = pairs
            .iter()
            .map(|(text, vector)| ((*text).to_owned(), vector.clone()))
            .collect();
        Self { dim, vectors }
    }
}

#[cfg(test)]
impl Embedder for MapEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        Ok(texts
            .iter()
            .map(|text| {
                self.vectors.get(text).cloned().unwrap_or_else(|| {
                    let mut vector = vec![0.0; self.dim];
                    if self.dim > 0 {
                        vector[0] = 1.0;
                    }
                    vector
                })
            })
            .collect())
    }
}
