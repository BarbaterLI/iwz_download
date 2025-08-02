use crate::config::Config;
use crate::extensions::Extension;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct SpeedLimitExtension {
    max_speed: u64, // bytes per second
    downloaded: Arc<AtomicU64>,
    last_check: Arc<Mutex<Instant>>,
}

impl SpeedLimitExtension {
    pub fn new(max_speed: u64) -> Self {
        Self {
            max_speed,
            downloaded: Arc::new(AtomicU64::new(0)),
            last_check: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub async fn limit_speed(&self, bytes: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.downloaded.fetch_add(bytes, Ordering::Relaxed);
        
        let now = Instant::now();
        let mut last_check = self.last_check.lock().await;
        let elapsed = now.duration_since(*last_check).as_secs_f64();
        
        if elapsed >= 0.2 {
            let downloaded = self.downloaded.swap(0, Ordering::Relaxed);
            let current_speed = (downloaded as f64 / elapsed) as u64;
            
            if current_speed > self.max_speed {
                let sleep_time = ((current_speed - self.max_speed) as f64 / self.max_speed as f64) * (elapsed * 1000.0);
                if sleep_time > 1.0 {
                    tokio::time::sleep(Duration::from_millis(sleep_time as u64)).await;
                }
            }
            
            *last_check = now;
        }
        
        Ok(())
    }
}

#[async_trait]
impl Extension for SpeedLimitExtension {
    async fn on_pre_download(&self, _config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Speed limit extension activated: {} KB/s", self.max_speed / 1024);
        Ok(())
    }

    async fn on_post_download(&self, _config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Speed limit extension completed");
        Ok(())
    }
}
