use std::process::Command;

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
    assert!(stdout.contains("mode=file "));
    assert!(stdout.contains("mode=ram "));
    assert!(stdout.contains("encode_mib_s="));
    assert!(stdout.contains("decode_mib_s="));
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
    assert!(stdout.contains("mode=zram "));
    assert!(stdout.contains("zram_zero_pages="));
    assert!(stdout.contains("zram_same_pages="));
    assert!(stdout.contains("zram_sxrc_pages="));
    assert!(stdout.contains("zram_raw_pages="));
}
