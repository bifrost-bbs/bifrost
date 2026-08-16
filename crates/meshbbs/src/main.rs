//! MeshBBS Host binary entry point.

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut config_path = PathBuf::from("config.toml");
    let mut cli_log_level: Option<String> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    config_path = PathBuf::from(&args[i + 1]);
                    i += 1;
                }
            }
            "--log-level" | "-l" => {
                if i + 1 < args.len() {
                    cli_log_level = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--debug" | "-v" | "--verbose" => {
                cli_log_level = Some("debug".to_string());
            }
            "--trace" => {
                cli_log_level = Some("trace".to_string());
            }
            "--help" | "-h" => {
                println!("Usage: meshbbs [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -c, --config <PATH>      Path to config file [default: config.toml]");
                println!("  -l, --log-level <LEVEL>  Set log level (trace, debug, info, warn, error)");
                println!("  -v, --debug, --verbose   Enable debug logging");
                println!("      --trace              Enable trace logging");
                println!("      --mock               Enable mock radio transport");
                println!("  -h, --help               Print help");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    // Determine effective log level precedence:
    // 1. Explicit CLI argument
    // 2. RUST_LOG environment variable
    // 3. log_level in config file
    // 4. Default: "info"
    let default_level = if let Some(ref lvl) = cli_log_level {
        lvl.clone()
    } else if std::env::var("RUST_LOG").is_ok() {
        "".to_string()
    } else if config_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = toml::from_str::<meshbbs::AppConfig>(&contents) {
                cfg.log_level
            } else {
                "info".to_string()
            }
        } else {
            "info".to_string()
        }
    } else {
        "info".to_string()
    };

    if !default_level.is_empty() {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level)).init();
    } else {
        env_logger::init();
    }

    // Delegate to the core runner
    meshbbs::run_bbs(Some(config_path), None).await
}
