use crate::config::Config;
use crate::task::{DownloadProgress, DownloadTask, TaskProgress};
use crate::extensions::Extension;
use crate::utils::{get_file_size, format_bytes};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, RANGE};
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

#[async_trait]
pub trait ProgressReporter: Send + Sync {
    async fn report_progress(&self, progress: DownloadProgress);
}

pub struct ConsoleProgressReporter;

#[async_trait]
impl ProgressReporter for ConsoleProgressReporter {
    async fn report_progress(&self, progress: DownloadProgress) {
        let percentage = progress.overall_progress();
        let downloaded = format_bytes(progress.total_downloaded);
        let total = format_bytes(progress.total_size);
        let speed = format_bytes(progress.speed as u64);
        
        print!(
            "\r[{}%] {} / {} @ {}/s",
            percentage as u32,
            downloaded,
            total,
            speed
        );
        std::io::stdout().flush().unwrap();
    }
}

pub struct Downloader {
    config: Config,
    extensions: Vec<Box<dyn Extension>>,
    progress_reporter: Option<Arc<dyn ProgressReporter>>,
}

impl Downloader {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            extensions: Vec::new(),
            progress_reporter: None,
        }
    }

    pub fn add_extension(&mut self, extension: Box<dyn Extension>) {
        self.extensions.push(extension);
    }

    pub fn set_progress_reporter(&mut self, reporter: Arc<dyn ProgressReporter>) {
        self.progress_reporter = Some(reporter);
    }

    pub async fn download(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting download from: {}", self.config.url);
        
        // Run pre-download extensions
        for extension in &self.extensions {
            extension.on_pre_download(&self.config).await?;
        }

        // Get file information
        let client = reqwest::Client::new();
        let head_response = client.head(&self.config.url).send().await?;
        let total_size = head_response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        if total_size == 0 {
            return Err("Unable to determine file size".into());
        }

        info!("File size: {} bytes", total_size);

        // Check if we can resume
        let resume_supported = head_response
            .headers()
            .get("accept-ranges")
            .map(|v| v == "bytes")
            .unwrap_or(false);

        if !resume_supported && self.config.resume {
            warn!("Server does not support range requests, resume disabled");
        }

        // Create output file
        let output_path = &self.config.output_path;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Calculate chunk sizes
        let chunk_size = total_size / self.config.thread_count as u64;
        let mut tasks = Vec::new();

        for i in 0..self.config.thread_count {
            let start = i as u64 * chunk_size;
            let end = if i == self.config.thread_count - 1 {
                total_size - 1
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            tasks.push(DownloadTask::new(
                self.config.url.clone(),
                self.config.output_path.clone(),
                start,
                end,
                i,
            ));
        }

        // Initialize progress tracking
        let progress = Arc::new(Mutex::new(DownloadProgress::new(total_size)));
        let (tx, mut rx) = mpsc::unbounded_channel::<TaskProgress>();

        // Start progress reporting
        let progress_clone = progress.clone();
        let reporter = self.progress_reporter.clone().unwrap_or_else(|| {
            Arc::new(ConsoleProgressReporter)
        });

        let progress_handle = tokio::spawn(async move {
            while let Some(task_progress) = rx.recv().await {
                let mut progress = progress_clone.lock().await;
                
                // Update task progress
                if let Some(existing) = progress.task_progress.iter_mut().find(|p| p.task_id == task_progress.task_id) {
                    *existing = task_progress.clone();
                } else {
                    progress.task_progress.push(task_progress.clone());
                }

                // Recalculate overall progress
                progress.total_downloaded = progress.task_progress.iter().map(|p| p.downloaded).sum();
                
                // Calculate speed and ETA
                // In a real implementation, we'd track history for better calculations
                progress.speed = progress.total_downloaded as f64;
                if progress.speed > 0.0 {
                    let remaining = progress.total_size - progress.total_downloaded;
                    progress.eta = Some((remaining as f64 / progress.speed) as u64);
                }

                reporter.report_progress(progress.clone()).await;
            }
        });

        // Execute download tasks
        let mut handles = Vec::new();
        for task in tasks {
            let tx = tx.clone();
            let client = client.clone();
            let buffer_size = self.config.buffer_size;
            let max_retries = self.config.max_retries;
            
            let handle = tokio::spawn(async move {
                Self::download_chunk(client, task, tx, buffer_size, max_retries).await
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await??;
        }

        // Close progress channel and wait for reporting to finish
        drop(tx);
        progress_handle.await?;

        println!(); // New line after progress

        // Run post-download extensions
        for extension in &self.extensions {
            extension.on_post_download(&self.config).await?;
        }

        info!("Download completed");
        Ok(())
    }

    async fn download_chunk(
        client: reqwest::Client,
        task: DownloadTask,
        progress_tx: mpsc::UnboundedSender<TaskProgress>,
        buffer_size: usize,
        max_retries: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut retry_count = 0;
        let mut progress = TaskProgress::new(task.thread_id, 0, task.size());

        loop {
            match Self::download_chunk_once(&client, &task, &progress_tx, &mut progress, buffer_size).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    retry_count += 1;
                    if retry_count > max_retries {
                        error!("Task {} failed after {} retries: {}", task.thread_id, max_retries, e);
                        return Err(e);
                    }
                    warn!("Task {} failed, retrying ({}/{}): {}", task.thread_id, retry_count, max_retries, e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn download_chunk_once(
        client: &reqwest::Client,
        task: &DownloadTask,
        progress_tx: &mpsc::UnboundedSender<TaskProgress>,
        progress: &mut TaskProgress,
        buffer_size: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Starting download task {}: {}-{}", task.thread_id, task.start_byte, task.end_byte);

        let mut headers = HeaderMap::new();
        headers.insert(RANGE, format!("bytes={}-{}", task.start_byte, task.end_byte).parse()?);

        let response = client
            .get(&task.url)
            .headers(headers)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()).into());
        }

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&task.output_path)
            .await?;

        file.seek(SeekFrom::Start(task.start_byte)).await?;

        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;
        let total = task.size();
        let start_time = Instant::now();

        while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Update progress
            let elapsed = start_time.elapsed().as_secs_f64();
            progress.downloaded = downloaded;
            progress.speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
            
            if progress.speed > 0.0 {
                let remaining = total - downloaded;
                progress.eta = Some((remaining as f64 / progress.speed) as u64);
            }

            progress_tx.send(progress.clone())?;
        }

        file.flush().await?;
        debug!("Completed download task {}: {} bytes", task.thread_id, downloaded);
        Ok(())
    }
}