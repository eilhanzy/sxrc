mod support;

use std::collections::BTreeSet;
use std::fs;
use std::process::Command;
use support::{write_bytes, TestDir};
use sxrc::SxrcManifest;

#[test]
fn sxrc_train_is_deterministic_and_uses_low_ids_first() {
    let temp = TestDir::new("train-deterministic");
    let input_dir = temp.join("pages");
    fs::create_dir_all(&input_dir).expect("failed to create training corpus dir");

    write_bytes(
        &input_dir.join("000-page.bin"),
        &[
            0x00, 0x00, 0x00, 0x00, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x00, 0x00,
            0x00, 0x00,
        ],
    );
    write_bytes(
        &input_dir.join("001-page.bin"),
        &[
            0x00, 0x00, 0x00, 0x00, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x00, 0x00,
            0x34, 0x12,
        ],
    );
    write_bytes(
        &input_dir.join("002-page.bin"),
        &[
            0x00, 0x00, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
    );

    let output_a = temp.join("trained-a.yaml");
    let output_b = temp.join("trained-b.yaml");

    for output in [&output_a, &output_b] {
        let result = Command::new(env!("CARGO_BIN_EXE_sxrc-train"))
            .args([
                "--input-dir",
                input_dir.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
                "--page-size",
                "16",
                "--compression-unit",
                "16-bit",
                "--endian",
                "little",
                "--max-dict",
                "4",
                "--max-patterns",
                "2",
            ])
            .output()
            .expect("failed to run sxrc-train");

        assert!(
            result.status.success(),
            "sxrc-train failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    let yaml_a = fs::read_to_string(&output_a).expect("failed to read first manifest");
    let yaml_b = fs::read_to_string(&output_b).expect("failed to read second manifest");
    assert_eq!(yaml_a, yaml_b);

    let manifest = SxrcManifest::from_yaml_str(&yaml_a).expect("manifest should parse");
    assert_eq!(manifest.compression_unit, sxrc::CompressionUnit::U16);
    assert_eq!(manifest.endian, sxrc::Endian::Little);
    assert!(!manifest.static_dictionary.is_empty());
    assert_eq!(manifest.static_dictionary[0].id, 0);
    assert_eq!(manifest.static_dictionary[0].value, "0x0000");
    assert!(manifest
        .static_dictionary
        .windows(2)
        .all(|pair| pair[0].id < pair[1].id));

    let dict_values = manifest
        .static_dictionary
        .iter()
        .map(|entry| entry.value.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(dict_values.len(), manifest.static_dictionary.len());

    assert!(!manifest.instruction_patterns.is_empty());
    assert_eq!(manifest.instruction_patterns[0].id, 0);
    assert!(manifest
        .instruction_patterns
        .windows(2)
        .all(|pair| pair[0].id < pair[1].id));

    let pattern_bytes = manifest
        .instruction_patterns
        .iter()
        .map(|pattern| pattern.hex_pattern.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(pattern_bytes.len(), manifest.instruction_patterns.len());
}

#[test]
fn sxrc_train_output_can_drive_sxrc_bench() {
    let temp = TestDir::new("train-bench-integration");
    let input_dir = temp.join("pages");
    fs::create_dir_all(&input_dir).expect("failed to create training corpus dir");

    write_bytes(
        &input_dir.join("000-page.bin"),
        &[
            0x07, 0xFE, 0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89, 0xD8, 0x55, 0x07, 0xFE,
            0x04, 0xEE,
        ],
    );
    write_bytes(
        &input_dir.join("001-page.bin"),
        &[
            0x07, 0xFE, 0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89, 0xD8, 0x55, 0x90, 0x90,
            0x48, 0x89,
        ],
    );

    let trained_manifest = temp.join("trained.yaml");
    let train = Command::new(env!("CARGO_BIN_EXE_sxrc-train"))
        .args([
            "--input-dir",
            input_dir.to_str().unwrap(),
            "--output",
            trained_manifest.to_str().unwrap(),
            "--page-size",
            "16",
            "--compression-unit",
            "16-bit",
            "--endian",
            "big",
            "--max-dict",
            "4",
            "--max-patterns",
            "2",
        ])
        .output()
        .expect("failed to run sxrc-train");

    assert!(
        train.status.success(),
        "sxrc-train failed: {}",
        String::from_utf8_lossy(&train.stderr)
    );

    let bench = Command::new(env!("CARGO_BIN_EXE_sxrc-bench"))
        .args([
            "--manifest",
            trained_manifest.to_str().unwrap(),
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
        .expect("failed to run sxrc-bench with trained manifest");

    assert!(
        bench.status.success(),
        "sxrc-bench failed: {}",
        String::from_utf8_lossy(&bench.stderr)
    );

    let stdout = String::from_utf8(bench.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("mode=ram codec=sxrc "));
}
