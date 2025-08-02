use crate::config::Config;
use crate::task::{DownloadProgress, DownloadTask, TaskProgress};
use crate::extensions::Extension;
use crate::utils::format_bytes;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, RANGE};
use std::fs::{self};
use std::io::{SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

#[async_trait]
pub trait ProgressReporter: Send + Sync {
    async fn report_progress(&self, progress: DownloadProgress);
}

pub struct ConsoleProgressReporter;

#[async_trait]
impl ProgressReporter for ConsoleProgressReporter {
    async fn report_progress(&self, progress: DownloadProgress) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static LAST_PRINTED: AtomicU64 = AtomicU64::new(0);
        let percentage = progress.overall_progress();
        let downloaded = format_bytes(progress.total_downloaded);
        let total = format_bytes(progress.total_size);
        let speed = format_bytes(progress.speed as u64);
        let eta = progress.eta.map(|s| format!(" ETA: {}s", s)).unwrap_or_default();

        // 只在进度有变化时刷新
        let percent_u64 = percentage as u64;
        if LAST_PRINTED.swap(percent_u64, Ordering::Relaxed) != percent_u64 || percent_u64 == 100 {
            print!(
                "\r[{:3.0}%] {} / {} @ {}/s{}",
                percentage,
                downloaded,
                total,
                speed,
                eta
            );
            std::io::stdout().flush().unwrap();
        }
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

    pub async fn download(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

        // 均匀分配 chunk，最后一个 chunk 只补余数
        let mut tasks = Vec::new();
        let mut start = 0u64;
        let mut remain = total_size;
        for i in 0..self.config.thread_count {
            let mut chunk = remain / (self.config.thread_count - i) as u64;
            if remain % (self.config.thread_count - i) as u64 != 0 {
                chunk += 1;
            }
            let end = (start + chunk - 1).min(total_size - 1);
            tasks.push(DownloadTask::new(
                self.config.url.clone(),
                self.config.output_path.clone(),
                start,
                end,
                i,
            ));
            start = end + 1;
            remain = remain.saturating_sub(chunk);
        }

        // Initialize progress tracking
        let progress = Arc::new(Mutex::new(DownloadProgress::new(total_size)));
        let (tx, mut rx) = mpsc::unbounded_channel::<TaskProgress>();

        // Start progress reporting
        let progress_clone = progress.clone();
        let reporter = self.progress_reporter.clone().unwrap_or_else(|| {
            Arc::new(ConsoleProgressReporter)
        });

        // 速度滑动窗口
        let progress_handle = tokio::spawn(async move {
            use std::collections::VecDeque;
            let mut speed_window = VecDeque::with_capacity(10);
            let mut last_downloaded = 0u64;
            let mut last_time = Instant::now();

            while let Some(task_progress) = rx.recv().await {
                let mut progress = progress_clone.lock().await;

                if let Some(existing) = progress.task_progress.iter_mut().find(|p| p.task_id == task_progress.task_id) {
                    *existing = task_progress.clone();
                } else {
                    progress.task_progress.push(task_progress.clone());
                }
                progress.total_downloaded = progress.task_progress.iter().map(|p| p.downloaded).sum();

                // 滑动窗口速度统计
                let now = Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                if elapsed > 0.2 {
                    let delta = progress.total_downloaded.saturating_sub(last_downloaded);
                    speed_window.push_back((delta, elapsed));
                    if speed_window.len() > 10 {
                        speed_window.pop_front();
                    }
                    let (sum_bytes, sum_secs): (u64, f64) = speed_window.iter().fold((0, 0.0), |(b, s), (db, ds)| (b + db, s + ds));
                    progress.speed = if sum_secs > 0.0 { sum_bytes as f64 / sum_secs } else { 0.0 };
                    last_downloaded = progress.total_downloaded;
                    last_time = now;
                }

                if progress.speed > 0.0 {
                    let remaining = progress.total_size.saturating_sub(progress.total_downloaded);
                    progress.eta = Some((remaining as f64 / progress.speed) as u64);
                } else {
                    progress.eta = None;
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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let mut offset = 0;
            while offset < chunk.len() {
                let end = (offset + buffer_size).min(chunk.len());
                file.write_all(&chunk[offset..end]).await?;
                offset = end;
            }
            downloaded += chunk.len() as u64;

            // Update progress
            let elapsed = start_time.elapsed().as_secs_f64();
            progress.downloaded = downloaded;
            progress.speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };

            if progress.speed > 0.0 {
                let remaining = total.saturating_sub(downloaded);
                progress.eta = Some((remaining as f64 / progress.speed) as u64);
            }

            progress_tx.send(progress.clone())?;
        }

        file.flush().await?;
        debug!("Completed download task {}: {} bytes", task.thread_id, downloaded);
        Ok(())
    }
}