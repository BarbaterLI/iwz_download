use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub url: String,
    pub output_path: PathBuf,
    pub thread_count: usize,
    pub buffer_size: usize,
    pub resume: bool,
    pub max_retries: usize,
}

impl Config {
    pub fn new(url: String, output_path: PathBuf) -> Self {
        Self {
            url,
            output_path,
            thread_count: 4,
            buffer_size: 8192,
            resume: true,
            max_retries: 3,
        }
    }
}