use crate::config::Config;
use crate::extensions::Extension;
use crate::utils::get_file_size;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};

pub struct ChecksumExtension {
    algorithm: String,
}

impl ChecksumExtension {
    pub fn new(algorithm: &str) -> Self {
        Self {
            algorithm: algorithm.to_string(),
        }
    }

    fn calculate_checksum(&self, file_path: &std::path::Path) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match self.algorithm.as_str() {
            "sha256" => {
                let mut file = File::open(file_path)?;
                let mut hasher = Sha256::new();
                let mut buffer = [0; 8192];
                let total = crate::utils::get_file_size(file_path)?;
                let mut read = 0u64;
                while let Ok(bytes_read) = file.read(&mut buffer) {
                    if bytes_read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..bytes_read]);
                    read += bytes_read as u64;
                    if total > 0 && read % (1024 * 1024) == 0 {
                        print!("\rChecksum progress: {:.1}%", (read as f64 / total as f64) * 100.0);
                        std::io::stdout().flush()?;
                    }
                }
                if total > 0 {
                    println!("\rChecksum progress: 100.0%");
                }
                let result = hasher.finalize();
                Ok(format!("{:x}", result))
            }
            _ => Err(format!("Unsupported checksum algorithm: {}", self.algorithm).into())
        }
    }
}

#[async_trait]
impl Extension for ChecksumExtension {
    async fn on_pre_download(&self, _config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Checksum verification extension activated ({})", self.algorithm);
        Ok(())
    }

    async fn on_post_download(&self, config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Calculating checksum...");
        
        let file_size = get_file_size(&config.output_path)?;
        if file_size == 0 {
            println!("Warning: Empty file, skipping checksum");
            return Ok(());
        }
        
        let checksum = self.calculate_checksum(&config.output_path)?;
        println!("File checksum ({}): {}", self.algorithm, checksum);
        
        Ok(())
    }
}
