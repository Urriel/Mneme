use fastembed::{EmbeddingModel, InitOptionsWithLength, TextEmbedding};

#[derive(Debug)]
pub(crate) struct EmbedError(String);

impl EmbedError {
    pub(crate) fn new(err: impl std::fmt::Display) -> Self {
        Self(err.to_string())
    }

    pub(crate) fn into_message(self) -> String {
        self.0
    }
}

pub(crate) struct Vector(Vec<f32>);

impl Vector {
    pub(crate) fn new(values: Vec<f32>) -> Result<Self, EmbedError> {
        if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
            return Err(EmbedError::new("embedding is empty or not finite"));
        }
        Ok(Self(values))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn to_vec(&self) -> Vec<f32> {
        self.0.clone()
    }
}

pub(crate) trait Embedder: Send {
    #[cfg(test)]
    fn id(&self) -> &str;
    #[cfg(test)]
    fn dimensions(&self) -> usize;
    fn embed(&mut self, text: &str) -> Result<Vector, EmbedError>;
}

pub(crate) struct FastEmbedder {
    model: TextEmbedding,
    dimensions: usize,
}

impl FastEmbedder {
    pub(crate) const ID: &'static str = "AllMiniLML6V2";

    pub(crate) fn default_dimensions() -> Result<usize, EmbedError> {
        let info = TextEmbedding::get_model_info(&EmbeddingModel::AllMiniLML6V2)
            .map_err(EmbedError::new)?;
        Ok(info.dim)
    }

    pub(crate) fn load() -> Result<Self, EmbedError> {
        let dimensions = Self::default_dimensions()?;
        let options = InitOptionsWithLength::new(EmbeddingModel::AllMiniLML6V2)
            .with_show_download_progress(false);
        let model = TextEmbedding::try_new(options).map_err(EmbedError::new)?;
        Ok(Self { model, dimensions })
    }
}

impl Embedder for FastEmbedder {
    #[cfg(test)]
    fn id(&self) -> &str {
        Self::ID
    }

    #[cfg(test)]
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&mut self, text: &str) -> Result<Vector, EmbedError> {
        let mut embeddings = self.model.embed([text], None).map_err(EmbedError::new)?;
        let values = embeddings
            .pop()
            .ok_or_else(|| EmbedError::new("embedder returned no vector"))?;
        if values.len() != self.dimensions {
            return Err(EmbedError::new(format!(
                "embedder returned width {}, expected {}",
                values.len(),
                self.dimensions
            )));
        }
        Vector::new(values)
    }
}

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
pub(crate) fn fake(pairs: &[(&str, [f32; 4])]) -> Box<dyn Embedder> {
    Box::new(MapEmbedder::from_pairs(pairs))
}

#[cfg(test)]
struct MapEmbedder {
    vectors: HashMap<String, Vec<f32>>,
}

#[cfg(test)]
impl MapEmbedder {
    fn from_pairs(pairs: &[(&str, [f32; 4])]) -> Self {
        let vectors = pairs
            .iter()
            .map(|(text, vector)| ((*text).to_owned(), vector.to_vec()))
            .collect();
        Self { vectors }
    }
}

#[cfg(test)]
impl Embedder for MapEmbedder {
    fn id(&self) -> &str {
        "fake"
    }

    fn dimensions(&self) -> usize {
        4
    }

    fn embed(&mut self, text: &str) -> Result<Vector, EmbedError> {
        let values = self
            .vectors
            .get(text)
            .cloned()
            .ok_or_else(|| EmbedError::new(format!("no vector for {text:?}")))?;
        Vector::new(values)
    }
}
