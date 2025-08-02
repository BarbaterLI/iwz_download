mod config;
mod downloader;
mod task;
mod utils;
mod extensions;

use clap::Parser;
use config::Config;
use downloader::Downloader;
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// URL to download
    #[arg(short, long)]
    url: String,

    /// Output file path
    #[arg(short, long, default_value = "output")]
    output: PathBuf,

    /// Number of concurrent threads
    #[arg(short, long, default_value_t = 4)]
    threads: usize,

    /// Buffer size in bytes
    #[arg(long, default_value_t = 8192)]
    buffer_size: usize,

    /// Enable verbose logging
    #[arg(short, long, action)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose {
        Level::INFO
    } else {
        Level::WARN
    };
    
    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();

    info!("Starting multithread downloader");

    let config = Config {
        url: args.url,
        output_path: args.output,
        thread_count: args.threads,
        buffer_size: args.buffer_size,
        resume: true,
        max_retries: 3,
    };

    let mut downloader = Downloader::new(config);
    
    // Add extensions
    downloader.add_extension(Box::new(extensions::speed_limit::SpeedLimitExtension::new(1024 * 1024))); // 1MB/s
    downloader.add_extension(Box::new(extensions::checksum::ChecksumExtension::new("sha256")));

    match downloader.download().await {
        Ok(_) => {
            info!("Download completed successfully");
            println!("Download completed successfully!");
        }
        Err(e) => {
            eprintln!("Download failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}