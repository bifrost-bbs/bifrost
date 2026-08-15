//! MeshBBS Host binary entry point.

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize Logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    
    // 2. Delegate to the core runner
    meshbbs::run_bbs(Some(PathBuf::from("config.toml")), None).await
}
