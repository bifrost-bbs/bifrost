//! Bifrost BBS Host binary entry point.

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut config_path = PathBuf::from("config.toml");
    let mut cli_log_level: Option<String> = None;

    let mut cli_capture_dir: Option<String> = None;

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
            "--capture-packets" | "--capture" | "--dump-packets" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    cli_capture_dir = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    cli_capture_dir = Some("captured_packets".to_string());
                }
            }
            "--debug" | "-v" | "--verbose" => {
                cli_log_level = Some("debug".to_string());
            }
            "--trace" => {
                cli_log_level = Some("trace".to_string());
            }
            "--help" | "-h" => {
                println!("Usage: bifrost-bbs [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -c, --config <PATH>      Path to config file [default: config.toml]");
                println!(
                    "  -l, --log-level <LEVEL>  Set log level (trace, debug, info, warn, error)"
                );
                println!("      --capture-packets [DIR] Capture all in/out raw and compressed packets to CSV and .bin files [default: captured_packets]");
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
    let resolved_config = bifrost_bbs::find_workspace_path(config_path.to_str().unwrap_or(""));
    let default_level = if let Some(ref lvl) = cli_log_level {
        lvl.clone()
    } else if std::env::var("RUST_LOG").is_ok() {
        "".to_string()
    } else if resolved_config.exists() {
        if let Ok(contents) = std::fs::read_to_string(&resolved_config) {
            if let Ok(cfg) = toml::from_str::<bifrost_bbs::AppConfig>(&contents) {
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
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
            .init();
    } else {
        env_logger::init();
    }

    // Delegate to the core runner with capture options
    bifrost_bbs::run_bbs_with_capture(Some(config_path), None, cli_capture_dir).await
}
