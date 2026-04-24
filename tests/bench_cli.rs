mod support;

use serde_json::Value;
use std::fs;
use std::process::Command;
use support::{write_bytes, TestDir};

#[test]
fn sxrc_bench_runs_file_and_ram_modes() {
    let output = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--mode",
            "both",
            "--dataset",
            "mixed",
            "--size-bytes",
            "4096",
            "--iterations",
            "2",
            "--page-size",
            "512",
        ])
        .output()
        .expect("failed to run sxrc-bench");

    assert!(
        output.status.success(),
        "sxrc-bench failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("sxrc-bench arch="));
    assert!(stdout.contains("mode=file codec=sxrc "));
    assert!(stdout.contains("mode=ram codec=sxrc "));
    assert!(stdout.contains("encode_mib_s="));
    assert!(stdout.contains("decode_mib_s="));
    assert!(stdout.contains("encode_p95_us="));
    assert!(stdout.contains("decode_p99_us="));
}

#[test]
fn sxrc_bench_runs_zram_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--mode",
            "zram",
            "--dataset",
            "repeated",
            "--size-bytes",
            "4096",
            "--iterations",
            "2",
            "--page-size",
            "512",
        ])
        .output()
        .expect("failed to run sxrc-bench");

    assert!(
        output.status.success(),
        "sxrc-bench failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("mode=zram codec=sxrc "));
    assert!(stdout.contains("zram_zero_pages="));
    assert!(stdout.contains("zram_same_pages="));
    assert!(stdout.contains("zram_sxrc_pages="));
    assert!(stdout.contains("zram_raw_pages="));
}

#[test]
fn sxrc_bench_supports_input_dir_baselines_and_json_export() {
    let temp = TestDir::new("bench-input-dir");
    let input_dir = temp.join("pages");
    fs::create_dir_all(&input_dir).expect("failed to create page dir");
    write_bytes(&input_dir.join("000-zero.bin"), &[0_u8; 16]);
    write_bytes(&input_dir.join("001-same.bin"), &[0xAA_u8; 16]);
    write_bytes(
        &input_dir.join("002-pattern.bin"),
        &[
            0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89, 0xD8, 0x55, 0x07, 0xFE, 0x04, 0xEE,
            0x90, 0x90,
        ],
    );
    let report_path = temp.join("report.json");

    let output = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--input-dir",
            input_dir.to_str().unwrap(),
            "--mode",
            "zram",
            "--iterations",
            "1",
            "--warmup",
            "2",
            "--page-size",
            "16",
            "--baseline",
            "both",
            "--latency-stats",
            "--export-json",
            report_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sxrc-bench");

    assert!(
        output.status.success(),
        "sxrc-bench failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("input_source=directory"));
    assert!(stdout.contains("mode=zram codec=sxrc "));
    assert!(stdout.contains("mode=zram codec=zstd "));
    assert!(stdout.contains("mode=zram codec=lz4 "));
    assert!(stdout.contains("encode_p50_us="));

    let report = fs::read_to_string(&report_path).expect("failed to read JSON report");
    let json: Value = serde_json::from_str(&report).expect("benchmark JSON should be valid");
    assert_eq!(json["input_source"].as_str(), Some("directory"));
    assert_eq!(json["iterations"].as_u64(), Some(1));
    assert_eq!(json["warmup"].as_u64(), Some(2));

    let results = json["results"]
        .as_array()
        .expect("results should be an array");
    assert_eq!(results.len(), 3);

    let sxrc = results
        .iter()
        .find(|result| result["codec"].as_str() == Some("sxrc"))
        .expect("sxrc result should exist");
    assert_eq!(sxrc["zram_zero_pages"].as_u64(), Some(1));
    assert_eq!(sxrc["zram_same_pages"].as_u64(), Some(1));
    assert!(sxrc["encode_p95_us"].is_number());
    assert!(sxrc["decode_p99_us"].is_number());
}

#[test]
fn sxrc_bench_rejects_empty_input_dirs() {
    let temp = TestDir::new("bench-empty-dir");
    let input_dir = temp.join("pages");
    fs::create_dir_all(&input_dir).expect("failed to create empty page dir");

    let output = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--input-dir",
            input_dir.to_str().unwrap(),
            "--mode",
            "ram",
            "--iterations",
            "1",
            "--page-size",
            "16",
        ])
        .output()
        .expect("failed to run sxrc-bench");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("does not contain any *.bin pages"));
}

#[test]
fn sxrc_bench_rejects_oversized_input_pages() {
    let temp = TestDir::new("bench-oversized-page");
    let input_dir = temp.join("pages");
    fs::create_dir_all(&input_dir).expect("failed to create page dir");
    write_bytes(&input_dir.join("too-big.bin"), &[0x55_u8; 33]);

    let output = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--input-dir",
            input_dir.to_str().unwrap(),
            "--mode",
            "ram",
            "--iterations",
            "1",
            "--page-size",
            "32",
        ])
        .output()
        .expect("failed to run sxrc-bench");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("exceeds --page-size 32"));
}
