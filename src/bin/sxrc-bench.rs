use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sxrc::{
    SxrcCodecConfig, SxrcFileDecoder, SxrcFileEncoder, SxrcManifest, SxrcPageCodec, SxrcRamCodec,
};

const DEFAULT_MANIFEST_YAML: &str = r#"
version: "1.0-alpha"
target_arch: "generic"
compression_unit: "16-bit"
endian: "little"
static_dictionary:
  - id: 0x0001
    value: "0x0000"
  - id: 0x0002
    value: "0xFFFF"
  - id: 0x0003
    value: "0x07FE"
  - id: 0x0004
    value: "0x04EE"
  - id: 0x0005
    value: "0x9090"
instruction_patterns:
  - id: 0x0001
    mnemonic: "MOV_RAX_RBX"
    hex_pattern: "0x4889D8"
  - id: 0x0002
    mnemonic: "PUSH_RBP"
    hex_pattern: "0x55"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchMode {
    File,
    Ram,
    Zram,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchDataset {
    Zero,
    Same,
    Repeated,
    Mixed,
    Entropy,
}

#[derive(Debug, Clone)]
struct BenchArgs {
    manifest_path: Option<PathBuf>,
    input_path: Option<PathBuf>,
    mode: BenchMode,
    dataset: BenchDataset,
    size_bytes: usize,
    iterations: usize,
    page_size: usize,
    min_rle_units: usize,
    dynamic_patterns: bool,
}

#[derive(Debug, Clone, Copy)]
struct BenchMetrics {
    iterations: usize,
    input_bytes: usize,
    encoded_bytes: usize,
    encode_elapsed: Duration,
    decode_elapsed: Duration,
    dynamic_pattern_count: usize,
    dynamic_metadata_bytes: usize,
    zram_zero_pages: usize,
    zram_same_pages: usize,
    zram_sxrc_pages: usize,
    zram_raw_pages: usize,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("sxrc-bench: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let manifest = load_manifest(args.manifest_path.as_ref())?;
    let mut config = SxrcCodecConfig::from_manifest(&manifest);
    config.page_size = args.page_size;
    config.min_rle_units = args.min_rle_units;
    config.enable_dynamic_patterns = args.dynamic_patterns;

    let input = load_or_generate_input(&args)?;
    if input.is_empty() {
        return Err("benchmark input is empty".to_string());
    }

    println!(
        "sxrc-bench arch={} mode={:?} dataset={:?} input_bytes={} iterations={} page_size={} dynamic_patterns={}",
        std::env::consts::ARCH,
        args.mode,
        args.dataset,
        input.len(),
        args.iterations,
        config.page_size,
        config.enable_dynamic_patterns
    );

    if matches!(args.mode, BenchMode::File | BenchMode::Both) {
        let metrics = bench_file_codec(&manifest, config, &input, args.iterations)?;
        print_metrics("file", metrics);
    }

    if matches!(args.mode, BenchMode::Ram | BenchMode::Both) {
        let metrics = bench_ram_codec(&manifest, config, &input, args.iterations)?;
        print_metrics("ram", metrics);
    }

    if matches!(args.mode, BenchMode::Zram) {
        let metrics = bench_zram_sim(&manifest, config, &input, args.iterations)?;
        print_metrics("zram", metrics);
    }

    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<BenchArgs, String> {
    let mut parsed = BenchArgs {
        manifest_path: None,
        input_path: None,
        mode: BenchMode::Both,
        dataset: BenchDataset::Mixed,
        size_bytes: 8 * 1024 * 1024,
        iterations: 16,
        page_size: 4096,
        min_rle_units: 3,
        dynamic_patterns: true,
    };

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--manifest" => {
                parsed.manifest_path = Some(PathBuf::from(next_arg(&mut args, "--manifest")?));
            }
            "--input" => {
                parsed.input_path = Some(PathBuf::from(next_arg(&mut args, "--input")?));
            }
            "--mode" => {
                parsed.mode = match next_arg(&mut args, "--mode")?.as_str() {
                    "file" => BenchMode::File,
                    "ram" => BenchMode::Ram,
                    "zram" => BenchMode::Zram,
                    "both" => BenchMode::Both,
                    value => {
                        return Err(format!(
                            "unsupported --mode '{value}', expected file|ram|zram|both"
                        ))
                    }
                };
            }
            "--dataset" => {
                parsed.dataset = match next_arg(&mut args, "--dataset")?.as_str() {
                    "zero" => BenchDataset::Zero,
                    "same" => BenchDataset::Same,
                    "repeated" => BenchDataset::Repeated,
                    "mixed" => BenchDataset::Mixed,
                    "entropy" => BenchDataset::Entropy,
                    value => {
                        return Err(format!(
                            "unsupported --dataset '{value}', expected zero|same|repeated|mixed|entropy"
                        ))
                    }
                };
            }
            "--size-bytes" => {
                parsed.size_bytes =
                    parse_usize("--size-bytes", &next_arg(&mut args, "--size-bytes")?)?;
            }
            "--iterations" => {
                parsed.iterations =
                    parse_usize("--iterations", &next_arg(&mut args, "--iterations")?)?;
            }
            "--page-size" => {
                parsed.page_size =
                    parse_usize("--page-size", &next_arg(&mut args, "--page-size")?)?;
            }
            "--min-rle-units" => {
                parsed.min_rle_units =
                    parse_usize("--min-rle-units", &next_arg(&mut args, "--min-rle-units")?)?;
            }
            "--disable-dynamic-patterns" => {
                parsed.dynamic_patterns = false;
            }
            value => return Err(format!("unknown argument '{value}'")),
        }
    }

    if parsed.iterations == 0 {
        return Err("--iterations must be > 0".to_string());
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

fn print_help() {
    println!(
        "Usage: sxrc-bench [--mode file|ram|zram|both] [--dataset zero|same|repeated|mixed|entropy] [--size-bytes N] [--iterations N] [--page-size N] [--min-rle-units N] [--manifest PATH] [--input PATH] [--disable-dynamic-patterns]"
    );
}

fn load_manifest(path: Option<&PathBuf>) -> Result<SxrcManifest, String> {
    let yaml = if let Some(path) = path {
        fs::read_to_string(path)
            .map_err(|err| format!("failed to read manifest {}: {err}", path.display()))?
    } else {
        DEFAULT_MANIFEST_YAML.to_string()
    };
    SxrcManifest::from_yaml_str(&yaml).map_err(|err| err.to_string())
}

fn load_or_generate_input(args: &BenchArgs) -> Result<Vec<u8>, String> {
    if let Some(path) = &args.input_path {
        return fs::read(path)
            .map_err(|err| format!("failed to read input {}: {err}", path.display()));
    }

    Ok(match args.dataset {
        BenchDataset::Zero => vec![0_u8; args.size_bytes],
        BenchDataset::Same => vec![0xAA_u8; args.size_bytes],
        BenchDataset::Repeated => generate_repeated_corpus(args.size_bytes),
        BenchDataset::Mixed => generate_mixed_corpus(args.size_bytes),
        BenchDataset::Entropy => generate_entropy_corpus(args.size_bytes),
    })
}

fn bench_file_codec(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    input: &[u8],
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let encoder = SxrcFileEncoder::new(manifest, config).map_err(|err| err.to_string())?;
    let decoder = SxrcFileDecoder::new(manifest, config).map_err(|err| err.to_string())?;

    let mut encoded_bytes = 0usize;
    let mut dynamic_pattern_count = 0usize;
    let mut dynamic_metadata_bytes = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;

    for _ in 0..iterations {
        let encode_started = Instant::now();
        let encoded = encoder.encode(input).map_err(|err| err.to_string())?;
        encode_elapsed += encode_started.elapsed();

        encoded_bytes = encoded.bytes.len();
        dynamic_pattern_count = encoded.stats.dynamic_pattern_count;
        dynamic_metadata_bytes = encoded.stats.dynamic_metadata_bytes;
        black_box(encoded.stats);

        let decode_started = Instant::now();
        let decoded = decoder
            .decode(&encoded.bytes)
            .map_err(|err| err.to_string())?;
        decode_elapsed += decode_started.elapsed();

        if decoded != input {
            return Err("file codec round-trip mismatch".to_string());
        }
        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count,
        dynamic_metadata_bytes,
        zram_zero_pages: 0,
        zram_same_pages: 0,
        zram_sxrc_pages: 0,
        zram_raw_pages: 0,
    })
}

fn bench_ram_codec(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    input: &[u8],
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let codec = SxrcRamCodec::new(manifest, config).map_err(|err| err.to_string())?;
    let chunks = input.chunks(config.page_size).collect::<Vec<_>>();
    let mut encoded_bytes = 0usize;
    let mut dynamic_pattern_count = 0usize;
    let mut dynamic_metadata_bytes = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;

    for _ in 0..iterations {
        let encode_started = Instant::now();
        let pages = chunks
            .iter()
            .map(|chunk| codec.compress_page(chunk).map_err(|err| err.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        encode_elapsed += encode_started.elapsed();

        encoded_bytes = pages.iter().map(|page| page.encoded.len()).sum();
        dynamic_pattern_count = pages
            .iter()
            .map(|page| page.stats.dynamic_pattern_count)
            .sum();
        dynamic_metadata_bytes = pages
            .iter()
            .map(|page| page.stats.dynamic_metadata_bytes)
            .sum();
        black_box(encoded_bytes);

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &pages {
            let bytes = codec.decompress_page(page).map_err(|err| err.to_string())?;
            decoded.extend_from_slice(&bytes);
        }
        decode_elapsed += decode_started.elapsed();

        if decoded != input {
            return Err("RAM codec round-trip mismatch".to_string());
        }
        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count,
        dynamic_metadata_bytes,
        zram_zero_pages: 0,
        zram_same_pages: 0,
        zram_sxrc_pages: 0,
        zram_raw_pages: 0,
    })
}

fn bench_zram_sim(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    input: &[u8],
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let codec = SxrcRamCodec::new(manifest, config).map_err(|err| err.to_string())?;
    let chunks = input.chunks(config.page_size).collect::<Vec<_>>();
    let mut encoded_bytes = 0usize;
    let mut dynamic_pattern_count = 0usize;
    let mut dynamic_metadata_bytes = 0usize;
    let mut zram_zero_pages = 0usize;
    let mut zram_same_pages = 0usize;
    let mut zram_sxrc_pages = 0usize;
    let mut zram_raw_pages = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;

    for _ in 0..iterations {
        let mut encoded_pages = Vec::with_capacity(chunks.len());
        let encode_started = Instant::now();

        for chunk in &chunks {
            if chunk.iter().all(|byte| *byte == 0) {
                zram_zero_pages += 1;
                encoded_pages.push(ZramPage::Zero {
                    original_len: chunk.len(),
                });
                continue;
            }

            if let Some(fill_byte) = same_filled_byte(chunk) {
                zram_same_pages += 1;
                encoded_bytes += 1;
                encoded_pages.push(ZramPage::Same {
                    value: fill_byte,
                    original_len: chunk.len(),
                });
                continue;
            }

            let page = codec.compress_page(chunk).map_err(|err| err.to_string())?;
            encoded_bytes += page.encoded.len();
            dynamic_pattern_count += page.stats.dynamic_pattern_count;
            dynamic_metadata_bytes += page.stats.dynamic_metadata_bytes;
            match page.codec {
                SxrcPageCodec::Sxrc => zram_sxrc_pages += 1,
                SxrcPageCodec::Raw => zram_raw_pages += 1,
            }
            encoded_pages.push(ZramPage::Data(page));
        }

        encode_elapsed += encode_started.elapsed();

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &encoded_pages {
            match page {
                ZramPage::Zero { original_len } => {
                    decoded.resize(decoded.len() + *original_len, 0);
                }
                ZramPage::Same {
                    value,
                    original_len,
                } => {
                    decoded.resize(decoded.len() + *original_len, *value);
                }
                ZramPage::Data(page) => {
                    decoded.extend_from_slice(
                        &codec.decompress_page(page).map_err(|err| err.to_string())?,
                    );
                }
            }
        }
        decode_elapsed += decode_started.elapsed();

        if decoded != input {
            return Err("ZRAM simulation round-trip mismatch".to_string());
        }
        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes / iterations,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count: dynamic_pattern_count / iterations,
        dynamic_metadata_bytes: dynamic_metadata_bytes / iterations,
        zram_zero_pages: zram_zero_pages / iterations,
        zram_same_pages: zram_same_pages / iterations,
        zram_sxrc_pages: zram_sxrc_pages / iterations,
        zram_raw_pages: zram_raw_pages / iterations,
    })
}

fn print_metrics(mode: &str, metrics: BenchMetrics) {
    let input_mib = metrics.input_bytes as f64 / (1024.0 * 1024.0);
    let total_mib = input_mib * metrics.iterations as f64;
    println!(
        "mode={mode} ratio={:.6} encode_mib_s={:.2} decode_mib_s={:.2} encoded_bytes={} dynamic_patterns={} dynamic_metadata_bytes={} encode_ms_total={:.3} decode_ms_total={:.3} zram_zero_pages={} zram_same_pages={} zram_sxrc_pages={} zram_raw_pages={}",
        metrics.encoded_bytes as f64 / metrics.input_bytes as f64,
        throughput_mib_s(total_mib, metrics.encode_elapsed),
        throughput_mib_s(total_mib, metrics.decode_elapsed),
        metrics.encoded_bytes,
        metrics.dynamic_pattern_count,
        metrics.dynamic_metadata_bytes,
        metrics.encode_elapsed.as_secs_f64() * 1000.0,
        metrics.decode_elapsed.as_secs_f64() * 1000.0,
        metrics.zram_zero_pages,
        metrics.zram_same_pages,
        metrics.zram_sxrc_pages,
        metrics.zram_raw_pages,
    );
}

#[derive(Debug, Clone)]
enum ZramPage {
    Zero { original_len: usize },
    Same { value: u8, original_len: usize },
    Data(sxrc::SxrcCompressedPage),
}

fn throughput_mib_s(total_mib: f64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs == 0.0 {
        return f64::INFINITY;
    }
    total_mib / secs
}

fn generate_repeated_corpus(size_bytes: usize) -> Vec<u8> {
    fill_from_pattern(
        size_bytes,
        &[0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89, 0xD8, 0x55],
    )
}

fn generate_mixed_corpus(size_bytes: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size_bytes);
    let mut state = 0x1234_5678_9ABC_DEF0_u64;

    while output.len() < size_bytes {
        if (output.len() / 64).is_multiple_of(2) {
            output.extend_from_slice(&[0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89]);
        } else {
            state = xorshift64(state);
            output.extend_from_slice(&state.to_le_bytes());
        }
    }

    output.truncate(size_bytes);
    output
}

fn generate_entropy_corpus(size_bytes: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size_bytes);
    let mut state = 0xD1B5_4A32_D192_ED03_u64;

    while output.len() < size_bytes {
        state = xorshift64(state);
        output.extend_from_slice(&state.to_le_bytes());
    }

    output.truncate(size_bytes);
    output
}

fn fill_from_pattern(size_bytes: usize, pattern: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(size_bytes);
    while output.len() < size_bytes {
        output.extend_from_slice(pattern);
    }
    output.truncate(size_bytes);
    output
}

fn same_filled_byte(page: &[u8]) -> Option<u8> {
    let first = *page.first()?;
    page.iter().all(|byte| *byte == first).then_some(first)
}

fn xorshift64(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}
