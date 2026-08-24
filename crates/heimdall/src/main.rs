//! Heimdall CLI Entrypoint.

use anyhow::Result;
use heimdall::{find_workspace_root, HeimdallConfig, HeimdallServer};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut config = HeimdallConfig::default();
    let root = find_workspace_root();
    config.workspace_root = root.clone();
    config.config_path = root.join("config.toml");
    config.apps_dir = root.join("apps");
    config.capture_dir = root.join("captured_packets");

    let args: Vec<String> = std::env::args().collect();
    let mut idx = 1;

    while idx < args.len() {
        match args[idx].as_str() {
            "--port" | "-p" => {
                if idx + 1 < args.len() {
                    config.port = args[idx + 1].parse().unwrap_or(9324);
                    idx += 1;
                }
            }
            "--bind" | "-b" => {
                if idx + 1 < args.len() {
                    config.bind_addr = args[idx + 1].clone();
                    idx += 1;
                }
            }
            "--auth-token" => {
                if idx + 1 < args.len() {
                    config.auth.enabled = true;
                    config.auth.auth_token = Some(args[idx + 1].clone());
                    idx += 1;
                }
            }
            "--user" => {
                if idx + 1 < args.len() {
                    config.auth.enabled = true;
                    config.auth.username = Some(args[idx + 1].clone());
                    idx += 1;
                }
            }
            "--pass" => {
                if idx + 1 < args.len() {
                    config.auth.enabled = true;
                    config.auth.password = Some(args[idx + 1].clone());
                    idx += 1;
                }
            }
            "--web-dir" => {
                if idx + 1 < args.len() {
                    config.web_dir = Some(PathBuf::from(&args[idx + 1]));
                    idx += 1;
                }
            }
            "--config" | "-c" => {
                if idx + 1 < args.len() {
                    config.config_path = PathBuf::from(&args[idx + 1]);
                    idx += 1;
                }
            }
            "--no-auto-start" => {
                config.auto_start_bbs = false;
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                log::warn!("Unknown CLI argument: {}", other);
            }
        }
        idx += 1;
    }

    let server = HeimdallServer::new(config);
    server.run().await?;
    Ok(())
}

fn print_help() {
    println!(
        r#"Heimdall // Bifrost MeshBBS Master Supervisor & Web NOC

Usage:
  heimdall [OPTIONS]

Options:
  -p, --port <PORT>        Port to listen on [default: 9324]
  -b, --bind <ADDR>        Address to bind to [default: 0.0.0.0]
  -c, --config <PATH>      Path to BBS config.toml [default: config.toml]
      --web-dir <PATH>     Optional path to static web assets directory
      --auth-token <TOKEN> Enable Bearer token authentication
      --user <USER>        Enable HTTP Basic auth username
      --pass <PASS>        Enable HTTP Basic auth password
      --no-auto-start      Do not automatically spawn bifrost-bbs on boot
  -h, --help               Print help information
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_help_output() {
        print_help();
    }
}
