mod common;

use common::load_pages_from_dir;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use sxrc::{CompressionUnit, Endian, SxrcDictionaryEntry, SxrcInstructionPattern, SxrcManifest};

const MANIFEST_VERSION: &str = "1.0-alpha";
const TARGET_ARCH: &str = "generic";
const MAX_STATIC_PATTERN_LEN: usize = 16;

#[derive(Debug, Clone)]
struct TrainArgs {
    input_dir: PathBuf,
    output: PathBuf,
    page_size: usize,
    compression_unit: CompressionUnit,
    endian: Endian,
    max_dict: usize,
    max_patterns: usize,
}

#[derive(Debug, Clone)]
struct ScoredBytes {
    bytes: Vec<u8>,
    count: usize,
    score: usize,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("sxrc-train: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let pages = load_pages_from_dir(&args.input_dir, args.page_size)?;
    let manifest = train_manifest(&pages, &args)?;
    let yaml = manifest
        .to_yaml_string()
        .map_err(|err| format!("failed to serialize manifest: {err}"))?;
    fs::write(&args.output, yaml)
        .map_err(|err| format!("failed to write manifest {}: {err}", args.output.display()))?;

    println!(
        "sxrc-train pages={} compression_unit={:?} endian={:?} static_dictionary={} instruction_patterns={} output={}",
        pages.len(),
        args.compression_unit,
        args.endian,
        manifest.static_dictionary.len(),
        manifest.instruction_patterns.len(),
        args.output.display(),
    );
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<TrainArgs, String> {
    let mut parsed = TrainArgs {
        input_dir: PathBuf::new(),
        output: PathBuf::new(),
        page_size: 4096,
        compression_unit: CompressionUnit::U16,
        endian: Endian::Little,
        max_dict: 64,
        max_patterns: 32,
    };

    let mut saw_input_dir = false;
    let mut saw_output = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--input-dir" => {
                parsed.input_dir = PathBuf::from(next_arg(&mut args, "--input-dir")?);
                saw_input_dir = true;
            }
            "--output" => {
                parsed.output = PathBuf::from(next_arg(&mut args, "--output")?);
                saw_output = true;
            }
            "--page-size" => {
                parsed.page_size =
                    parse_usize("--page-size", &next_arg(&mut args, "--page-size")?)?;
            }
            "--compression-unit" => {
                parsed.compression_unit =
                    parse_compression_unit(&next_arg(&mut args, "--compression-unit")?)?;
            }
            "--endian" => {
                parsed.endian = parse_endian(&next_arg(&mut args, "--endian")?)?;
            }
            "--max-dict" => {
                parsed.max_dict = parse_usize("--max-dict", &next_arg(&mut args, "--max-dict")?)?;
            }
            "--max-patterns" => {
                parsed.max_patterns =
                    parse_usize("--max-patterns", &next_arg(&mut args, "--max-patterns")?)?;
            }
            value => return Err(format!("unknown argument '{value}'")),
        }
    }

    if !saw_input_dir {
        return Err("--input-dir is required".to_string());
    }
    if !saw_output {
        return Err("--output is required".to_string());
    }
    if parsed.page_size == 0 {
        return Err("--page-size must be > 0".to_string());
    }

    Ok(parsed)
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_usize(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|err| format!("invalid {flag} value '{value}': {err}"))
}

fn parse_compression_unit(value: &str) -> Result<CompressionUnit, String> {
    match value {
        "8-bit" => Ok(CompressionUnit::U8),
        "16-bit" => Ok(CompressionUnit::U16),
        "32-bit" => Ok(CompressionUnit::U32),
        _ => Err(format!(
            "unsupported --compression-unit '{value}', expected 8-bit|16-bit|32-bit"
        )),
    }
}

fn parse_endian(value: &str) -> Result<Endian, String> {
    match value {
        "little" => Ok(Endian::Little),
        "big" => Ok(Endian::Big),
        _ => Err(format!(
            "unsupported --endian '{value}', expected little|big"
        )),
    }
}

fn print_help() {
    println!(
        "Usage: sxrc-train --input-dir DIR --output PATH [--page-size N] [--compression-unit 8-bit|16-bit|32-bit] [--endian little|big] [--max-dict N] [--max-patterns N]"
    );
}

fn train_manifest(pages: &[Vec<u8>], args: &TrainArgs) -> Result<SxrcManifest, String> {
    let dictionary =
        select_dictionary_entries(pages, args.compression_unit, args.endian, args.max_dict);
    let patterns = select_pattern_entries(pages, args.compression_unit, args.max_patterns);

    let manifest = SxrcManifest {
        version: MANIFEST_VERSION.to_string(),
        target_arch: TARGET_ARCH.to_string(),
        compression_unit: args.compression_unit,
        endian: args.endian,
        static_dictionary: dictionary,
        instruction_patterns: patterns,
        memory_markers: BTreeMap::new(),
    };
    manifest
        .validate()
        .map_err(|err| format!("generated manifest is invalid: {err}"))?;
    Ok(manifest)
}

fn select_dictionary_entries(
    pages: &[Vec<u8>],
    unit: CompressionUnit,
    endian: Endian,
    max_dict: usize,
) -> Vec<SxrcDictionaryEntry> {
    if max_dict == 0 {
        return Vec::new();
    }

    let unit_len = unit.byte_len();
    let mut counts = BTreeMap::<Vec<u8>, usize>::new();
    for page in pages {
        for chunk in page.chunks_exact(unit_len) {
            *counts.entry(chunk.to_vec()).or_default() += 1;
        }
    }

    let mut candidates = counts
        .into_iter()
        .map(|(bytes, count)| ScoredBytes {
            score: count.saturating_mul(unit_len),
            bytes,
            count,
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.bytes.cmp(&right.bytes))
    });

    candidates
        .into_iter()
        .take(max_dict)
        .enumerate()
        .map(|(index, candidate)| SxrcDictionaryEntry {
            id: index as u32,
            value: format_dictionary_value(&candidate.bytes, unit, endian),
        })
        .collect()
}

fn select_pattern_entries(
    pages: &[Vec<u8>],
    unit: CompressionUnit,
    max_patterns: usize,
) -> Vec<SxrcInstructionPattern> {
    if max_patterns == 0 {
        return Vec::new();
    }

    let unit_len = unit.byte_len();
    let mut counts = BTreeMap::<Vec<u8>, usize>::new();
    for page in pages {
        let max_len = page.len().min(MAX_STATIC_PATTERN_LEN);
        for pattern_len in (unit_len * 2..=max_len)
            .filter(|len| len.is_multiple_of(unit_len))
            .collect::<Vec<_>>()
        {
            for offset in (0..=page.len() - pattern_len).step_by(unit_len) {
                let bytes = page[offset..offset + pattern_len].to_vec();
                if is_unit_rle_pattern(&bytes, unit_len) {
                    continue;
                }
                *counts.entry(bytes).or_default() += 1;
            }
        }
    }

    let mut candidates = counts
        .into_iter()
        .filter_map(|(bytes, count)| {
            if count < 2 {
                return None;
            }
            let literal_units = bytes.len() / unit_len;
            let savings_per_use = literal_units.saturating_mul(unit_len + 1).saturating_sub(1);
            let score = count.saturating_mul(savings_per_use);
            (score > 0).then_some(ScoredBytes {
                bytes,
                count,
                score,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.bytes.len().cmp(&left.bytes.len()))
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.bytes.cmp(&right.bytes))
    });

    candidates
        .into_iter()
        .take(max_patterns)
        .enumerate()
        .map(|(index, candidate)| SxrcInstructionPattern {
            id: index as u32,
            mnemonic: format!("AUTO_PATTERN_{index:04}"),
            hex_pattern: format_pattern_hex(&candidate.bytes),
        })
        .collect()
}

fn format_dictionary_value(bytes: &[u8], unit: CompressionUnit, endian: Endian) -> String {
    let value = match (unit, endian) {
        (CompressionUnit::U8, _) => bytes[0] as u64,
        (CompressionUnit::U16, Endian::Little) => u16::from_le_bytes([bytes[0], bytes[1]]) as u64,
        (CompressionUnit::U16, Endian::Big) => u16::from_be_bytes([bytes[0], bytes[1]]) as u64,
        (CompressionUnit::U32, Endian::Little) => {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64
        }
        (CompressionUnit::U32, Endian::Big) => {
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64
        }
    };
    format!("0x{value:0width$X}", width = unit.byte_len() * 2)
}

fn format_pattern_hex(bytes: &[u8]) -> String {
    let mut output = String::from("0x");
    for byte in bytes {
        output.push_str(&format!("{byte:02X}"));
    }
    output
}

fn is_unit_rle_pattern(bytes: &[u8], unit_len: usize) -> bool {
    let first = &bytes[..unit_len];
    bytes.chunks_exact(unit_len).all(|chunk| chunk == first)
}
