//! Bifrost Compression Tuning, Training, and Benchmark CLI tool.

use anyhow::{Context, Result};
use bifrost_compression::{
    compress_adaptive, CompressionDictionary, DictionaryTrainer, Heatshrink,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        print_help();
        return Ok(());
    }

    let command = args[1].as_str();
    match command {
        "analyze" | "benchmark" => run_analyze(&args[2..])?,
        "train" => run_train(&args[2..])?,
        "sweep" => run_sweep(&args[2..])?,
        _ => {
            // Default to analyze if first arg is a path
            if Path::new(command).exists() || command.starts_with("--dir") {
                run_analyze(&args[1..])?;
            } else {
                eprintln!("Unknown command: '{}'", command);
                print_help();
            }
        }
    }

    Ok(())
}

fn print_help() {
    println!("Bifrost Compression Tuning & Benchmark Tool");
    println!();
    println!("Usage:");
    println!("  bifrost-tuning <COMMAND> [OPTIONS]");
    println!();
    println!("Commands:");
    println!("  analyze   Benchmark and compare compression algorithms on captured packets");
    println!("  train     Train a custom BBS domain dictionary from captured packets");
    println!(
        "  sweep     Run parameter grid search across Heatshrink window and lookahead settings"
    );
    println!();
    println!("Options for 'analyze':");
    println!(
        "  --dir <PATH>   Path to captured raw packets directory [default: captured_packets/raw]"
    );
    println!();
    println!("Options for 'train':");
    println!("  --dir <PATH>      Path to captured raw packets directory [default: captured_packets/raw]");
    println!("  --out <PATH>      Output dictionary binary path [default: config/bbs_dict.bin]");
    println!("  --tokens <NUM>    Maximum tokens to train (1..254) [default: 128]");
    println!();
    println!("Options for 'sweep':");
    println!(
        "  --dir <PATH>   Path to captured raw packets directory [default: captured_packets/raw]"
    );
}

fn load_samples_from_dir(dir: &Path) -> Result<Vec<Vec<u8>>> {
    let mut samples = Vec::new();
    if !dir.exists() {
        anyhow::bail!("Directory does not exist: {:?}", dir);
    }

    let entries = fs::read_dir(dir).with_context(|| format!("Failed to read dir {:?}", dir))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |ext| ext == "bin"))
        .collect();
    paths.sort();

    for path in paths {
        let bytes = fs::read(&path).with_context(|| format!("Failed to read {:?}", path))?;
        if !bytes.is_empty() {
            samples.push(bytes);
        }
    }

    if samples.is_empty() {
        anyhow::bail!("No .bin packet files found in {:?}", dir);
    }

    Ok(samples)
}

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 < args.len() {
                return Some(args[i + 1].clone());
            }
        } else if args[i].starts_with(&format!("{}=", flag)) {
            return Some(args[i][flag.len() + 1..].to_string());
        }
        i += 1;
    }
    None
}

fn run_analyze(args: &[String]) -> Result<()> {
    let dir_str = parse_arg(args, "--dir").unwrap_or_else(|| "captured_packets/raw".to_string());
    let dir = PathBuf::from(&dir_str);
    let samples = load_samples_from_dir(&dir)?;

    let total_raw: usize = samples.iter().map(|s| s.len()).sum();
    let min_raw = samples.iter().map(|s| s.len()).min().unwrap_or(0);
    let max_raw = samples.iter().map(|s| s.len()).max().unwrap_or(0);
    let avg_raw = total_raw as f64 / samples.len() as f64;

    println!("================================================================================");
    println!(" BIFROST COMPRESSION BENCHMARK & ANALYSIS REPORT");
    println!("================================================================================");
    println!(
        " Dataset:       {:?} ({} sample packets)",
        dir,
        samples.len()
    );
    println!(
        " Total Raw:     {} bytes (Min: {} B, Avg: {:.1} B, Max: {} B)",
        total_raw, min_raw, avg_raw, max_raw
    );
    println!("--------------------------------------------------------------------------------");

    let sample_refs: Vec<&[u8]> = samples.iter().map(|v| v.as_slice()).collect();
    let static_dict = CompressionDictionary::standard_static();
    let trained_dict_128 = DictionaryTrainer::train_from_samples(&sample_refs, 128);
    let trained_dict_254 = DictionaryTrainer::train_from_samples(&sample_refs, 254);

    let algorithms: Vec<(&str, Box<dyn Fn(&[u8]) -> (u8, Vec<u8>)>)> = vec![
        (
            "1. Uncompressed (Raw Baseline)",
            Box::new(|data| (0x00, data.to_vec())),
        ),
        (
            "2. Heatshrink (W=8, L=4 - Default)",
            Box::new(|data| {
                let hs = Heatshrink::new(8, 4).unwrap();
                let comp = hs.compress(data).unwrap_or_else(|_| data.to_vec());
                (0x02, comp)
            }),
        ),
        (
            "3. Heatshrink (W=6, L=4 - Small Window)",
            Box::new(|data| {
                let hs = Heatshrink::new(6, 4).unwrap();
                let comp = hs.compress(data).unwrap_or_else(|_| data.to_vec());
                (0x02, comp)
            }),
        ),
        (
            "4. Heatshrink (W=7, L=4 - Tuned)",
            Box::new(|data| {
                let hs = Heatshrink::new(7, 4).unwrap();
                let comp = hs.compress(data).unwrap_or_else(|_| data.to_vec());
                (0x02, comp)
            }),
        ),
        (
            "5. Static Domain Dictionary Only",
            Box::new(|data| (0x04, static_dict.compress(data))),
        ),
        (
            "6. Static Dict + Heatshrink (W=8, L=4)",
            Box::new(|data| {
                let dict_comp = static_dict.compress(data);
                let hs = Heatshrink::new(8, 4).unwrap();
                let comp = hs.compress(&dict_comp).unwrap_or(dict_comp);
                (0x06, comp)
            }),
        ),
        (
            "7. Trained Dict (128 tokens) Only",
            Box::new(|data| (0x04, trained_dict_128.compress(data))),
        ),
        (
            "8. Trained Dict (128 tokens) + Heatshrink",
            Box::new(|data| {
                let dict_comp = trained_dict_128.compress(data);
                let hs = Heatshrink::new(8, 4).unwrap();
                let comp = hs.compress(&dict_comp).unwrap_or(dict_comp);
                (0x06, comp)
            }),
        ),
        (
            "9. Trained Dict (254 tokens) + Heatshrink",
            Box::new(|data| {
                let dict_comp = trained_dict_254.compress(data);
                let hs = Heatshrink::new(8, 4).unwrap();
                let comp = hs.compress(&dict_comp).unwrap_or(dict_comp);
                (0x06, comp)
            }),
        ),
        (
            "10. Adaptive Pipeline (Static Dict + Guard)",
            Box::new(|data| compress_adaptive(data, Some(&static_dict), 8, 4)),
        ),
        (
            "11. Adaptive Pipeline (Trained Dict + Guard)",
            Box::new(|data| compress_adaptive(data, Some(&trained_dict_128), 8, 4)),
        ),
    ];

    println!(
        "{:<44} | {:>9} | {:>7} | {:>9} | {:>8}",
        "Algorithm", "Out Bytes", "Ratio", "Savings %", "Avg Time"
    );
    println!("--------------------------------------------------------------------------------");

    for (name, func) in algorithms {
        let start = Instant::now();
        let mut total_out = 0;
        let mut expanded_count = 0;

        for s in &samples {
            let (_flags, out) = func(s);
            if out.len() > s.len() {
                expanded_count += 1;
            }
            total_out += out.len();
        }

        let elapsed = start.elapsed();
        let avg_time_us = elapsed.as_micros() as f64 / samples.len() as f64;
        let ratio = total_out as f64 / total_raw as f64;
        let savings_pct = ((total_raw as f64 - total_out as f64) / total_raw as f64) * 100.0;

        let expansion_note = if expanded_count > 0 {
            format!(" ({} expanded)", expanded_count)
        } else {
            "".to_string()
        };

        println!(
            "{:<44} | {:>7} B | {:>6.2}x | {:>+8.2}% | {:>6.1}µs{}",
            name, total_out, ratio, savings_pct, avg_time_us, expansion_note
        );
    }
    println!("================================================================================");

    Ok(())
}

fn run_sweep(args: &[String]) -> Result<()> {
    let dir_str = parse_arg(args, "--dir").unwrap_or_else(|| "captured_packets/raw".to_string());
    let dir = PathBuf::from(&dir_str);
    let samples = load_samples_from_dir(&dir)?;
    let total_raw: usize = samples.iter().map(|s| s.len()).sum();

    println!("================================================================================");
    println!(" HEATSHRINK LZSS PARAMETER GRID SEARCH");
    println!("================================================================================");
    println!(" Testing window size W in [4..11] and lookahead L in [3..6]");
    println!("--------------------------------------------------------------------------------");
    println!(
        "{:<8} | {:<10} | {:>9} | {:>7} | {:>9} | {:>10}",
        "Window", "Lookahead", "Out Bytes", "Ratio", "Savings %", "Expanded"
    );
    println!("--------------------------------------------------------------------------------");

    let mut best_config: Option<(u8, u8, usize, f64)> = None;

    for w in 4..=11 {
        for l in 3..=std::cmp::min(6, w) {
            if let Ok(hs) = Heatshrink::new(w, l) {
                let mut total_out = 0;
                let mut expanded = 0;

                for s in &samples {
                    let out = hs.compress(s).unwrap_or_else(|_| s.to_vec());
                    if out.len() > s.len() {
                        expanded += 1;
                    }
                    total_out += out.len();
                }

                let ratio = total_out as f64 / total_raw as f64;
                let savings_pct =
                    ((total_raw as f64 - total_out as f64) / total_raw as f64) * 100.0;

                println!(
                    "W={:<5} | L={:<8} | {:>7} B | {:>6.2}x | {:>+8.2}% | {:>10}",
                    w, l, total_out, ratio, savings_pct, expanded
                );

                if best_config
                    .as_ref()
                    .map_or(true, |(_, _, best_bytes, _)| total_out < *best_bytes)
                {
                    best_config = Some((w, l, total_out, savings_pct));
                }
            }
        }
    }

    if let Some((best_w, best_l, best_bytes, best_savings)) = best_config {
        println!(
            "--------------------------------------------------------------------------------"
        );
        println!(
            " OPTIMAL CONFIG: W={} ({}B window), L={} ({}B lookahead) -> {} bytes ({:+.2}% savings)",
            best_w,
            1 << best_w,
            best_l,
            1 << best_l,
            best_bytes,
            best_savings
        );
        println!(
            "================================================================================"
        );
    }

    Ok(())
}

fn run_train(args: &[String]) -> Result<()> {
    let dir_str = parse_arg(args, "--dir").unwrap_or_else(|| "captured_packets/raw".to_string());
    let out_str = parse_arg(args, "--out").unwrap_or_else(|| "config/bbs_dict.bin".to_string());
    let token_count: usize = parse_arg(args, "--tokens")
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);

    let dir = PathBuf::from(&dir_str);
    let samples = load_samples_from_dir(&dir)?;
    let sample_refs: Vec<&[u8]> = samples.iter().map(|v| v.as_slice()).collect();

    println!(
        "Training custom domain dictionary from {} samples (target: {} tokens)...",
        samples.len(),
        token_count
    );
    let dict = DictionaryTrainer::train_from_samples(&sample_refs, token_count);
    let bytes = dict.to_bytes();

    let out_path = PathBuf::from(&out_str);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, &bytes)?;

    println!(
        "Saved dictionary artifact ({} bytes, CRC32: 0x{:08X}) to {:?}",
        bytes.len(),
        dict.crc32(),
        out_path
    );
    println!("Dictionary contains {} tokens:", dict.tokens().len());
    for (i, t) in dict.tokens().iter().enumerate().take(25) {
        let display_str = String::from_utf8_lossy(t);
        println!("  #{:02X} ({}B): {:?}", i, t.len(), display_str);
    }
    if dict.tokens().len() > 25 {
        println!("  ... and {} more tokens", dict.tokens().len() - 25);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arg() {
        let args = vec![
            "--dir".to_string(),
            "foo/bar".to_string(),
            "--tokens=64".to_string(),
        ];
        assert_eq!(parse_arg(&args, "--dir"), Some("foo/bar".to_string()));
        assert_eq!(parse_arg(&args, "--tokens"), Some("64".to_string()));
        assert_eq!(parse_arg(&args, "--missing"), None);
    }

    #[test]
    fn test_tuning_workflow_with_synthetic_samples() {
        let temp_dir = std::env::temp_dir().join("bifrost_tuning_test_samples");
        let _ = std::fs::create_dir_all(&temp_dir);

        // Write test sample binary packets
        let sample1 = b"[MENU] (1) Messages (2) Marketplace (3) Dungeon";
        let sample2 = b"[MENU] (1) Messages (2) Marketplace (4) Profile";
        let sample3 = b"[FORM] Nickname: ________ [Submit]";
        std::fs::write(temp_dir.join("sample1.bin"), sample1).unwrap();
        std::fs::write(temp_dir.join("sample2.bin"), sample2).unwrap();
        std::fs::write(temp_dir.join("sample3.bin"), sample3).unwrap();

        // 1. Test load_samples_from_dir
        let samples = load_samples_from_dir(&temp_dir).expect("Should load samples");
        assert_eq!(samples.len(), 3);

        // 2. Test run_analyze
        let analyze_args = vec!["--dir".to_string(), temp_dir.to_string_lossy().to_string()];
        assert!(run_analyze(&analyze_args).is_ok());

        // 3. Test run_sweep
        let sweep_args = vec!["--dir".to_string(), temp_dir.to_string_lossy().to_string()];
        assert!(run_sweep(&sweep_args).is_ok());

        // 4. Test run_train
        let out_dict = temp_dir.join("test_dict.bin");
        let train_args = vec![
            "--dir".to_string(),
            temp_dir.to_string_lossy().to_string(),
            "--out".to_string(),
            out_dict.to_string_lossy().to_string(),
            "--tokens".to_string(),
            "10".to_string(),
        ];
        assert!(run_train(&train_args).is_ok());
        assert!(out_dict.exists(), "Trained dictionary file must be created");

        let dict_bytes = std::fs::read(&out_dict).unwrap();
        let loaded = CompressionDictionary::from_bytes(&dict_bytes)
            .expect("Should parse generated dictionary");
        assert!(!loaded.tokens().is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_samples_from_nonexistent_dir() {
        let non_dir = std::env::temp_dir().join("nonexistent_bifrost_dir_123");
        assert!(load_samples_from_dir(&non_dir).is_err());
    }

    #[test]
    fn test_print_help() {
        print_help();
    }
}
