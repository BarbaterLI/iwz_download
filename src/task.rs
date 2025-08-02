use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub url: String,
    pub output_path: PathBuf,
    pub start_byte: u64,
    pub end_byte: u64,
    pub thread_id: usize,
}

impl DownloadTask {
    pub fn new(url: String, output_path: PathBuf, start: u64, end: u64, thread_id: usize) -> Self {
        Self {
            url,
            output_path,
            start_byte: start,
            end_byte: end,
            thread_id,
        }
    }

    pub fn range(&self) -> (u64, u64) {
        (self.start_byte, self.end_byte)
    }

    pub fn size(&self) -> u64 {
        self.end_byte - self.start_byte + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task_id: usize,
    pub downloaded: u64,
    pub total: u64,
    pub speed: f64, // bytes per second
    pub eta: Option<u64>, // seconds
}

impl TaskProgress {
    pub fn new(task_id: usize, downloaded: u64, total: u64) -> Self {
        Self {
            task_id,
            downloaded,
            total,
            speed: 0.0,
            eta: None,
        }
    }

    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64) * 100.0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub total_downloaded: u64,
    pub total_size: u64,
    pub speed: f64,
    pub eta: Option<u64>,
    pub task_progress: Vec<TaskProgress>,
}

impl DownloadProgress {
    pub fn new(total_size: u64) -> Self {
        Self {
            total_downloaded: 0,
            total_size,
            speed: 0.0,
            eta: None,
            task_progress: Vec::new(),
        }
    }

    pub fn overall_progress(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            (self.total_downloaded as f64 / self.total_size as f64) * 100.0
        }
    }
}