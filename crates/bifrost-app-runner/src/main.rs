//! CLI Entry point for Bifrost App Runner.

use anyhow::Result;
use bifrost_app_runner::{AppRunner, RunnerConfig};
use std::path::PathBuf;

fn print_help() {
    println!(
        r#"Bifrost App Runner - Standalone Developer Runtime & Interactive Terminal

USAGE:
    bifrost-runner [OPTIONS] [APP_DIR]

ARGS:
    <APP_DIR>               Path to application directory containing manifest.toml (default: current directory)

OPTIONS:
    -u, --user <NICK>       Mock user nickname (default: "DevOperator")
    -n, --node-id <HEX>     Mock 64-hex node ID (default: "010101...01")
        --db <PATH>         Path to SQLite test database file (default: in-memory)
        --headless          Run in headless mode (non-interactive, outputs screen text)
    -h, --help              Print this help information
    -V, --version           Print version information

EXAMPLES:
    bifrost-runner
    bifrost-runner ./apps/starter
    bifrost-runner --user Alice --headless ./apps/weather
"#
    );
}

fn main() -> Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let mut config = RunnerConfig::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-V" | "--version" => {
                println!("bifrost-runner v{}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-u" | "--user" => {
                if i + 1 < args.len() {
                    config.user_nickname = args[i + 1].clone();
                    i += 1;
                }
            }
            "-n" | "--node-id" => {
                if i + 1 < args.len() {
                    config.user_node_id = args[i + 1].clone();
                    i += 1;
                }
            }
            "--db" => {
                if i + 1 < args.len() {
                    config.db_path = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--headless" => {
                config.headless = true;
            }
            arg if !arg.starts_with('-') => {
                config.app_dir = PathBuf::from(arg);
            }
            other => {
                eprintln!("Unknown option: {}", other);
                print_help();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let runner = AppRunner::new(config.clone())?;

    if config.headless {
        let lua = runner.setup_lua()?;
        runner.run_start(&lua)?;
        let screen_text = runner.screen.lock().unwrap().render_to_plain_text();
        println!("{}", screen_text);
    } else {
        runner.run_interactive()?;
    }

    Ok(())
}
