use std::path::PathBuf;

pub const DEFAULT_MODEL: &str = "Qwen/Qwen3-Embedding-0.6B";
pub const DEFAULT_DIM: usize = 1024;
pub const DEFAULT_DEVICE: &str = "cpu";
pub const DEFAULT_DATA_DIR: &str = "./mneme-data";

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub model: String,
    pub dim: usize,
    pub device: String,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            model: DEFAULT_MODEL.to_owned(),
            dim: DEFAULT_DIM,
            device: DEFAULT_DEVICE.to_owned(),
        }
    }
}
