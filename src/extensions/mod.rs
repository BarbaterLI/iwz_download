use crate::config::Config;
use async_trait::async_trait;

#[async_trait]
pub trait Extension: Send + Sync {
    async fn on_pre_download(&self, config: &Config) -> Result<(), Box<dyn std::error::Error>>;
    async fn on_post_download(&self, config: &Config) -> Result<(), Box<dyn std::error::Error>>;
}

pub mod speed_limit;
pub mod checksum;