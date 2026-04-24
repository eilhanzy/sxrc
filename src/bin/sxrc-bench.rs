mod common;

use common::{flatten_pages, load_pages_from_dir, InputSource, LoadedInput};
use lz4_flex::block;
use serde::Serialize;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sxrc::{
    SxrcCodecConfig, SxrcCompressedPage, SxrcFileDecoder, SxrcFileEncoder, SxrcManifest,
    SxrcPageCodec, SxrcRamCodec,
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

impl BenchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Ram => "ram",
            Self::Zram => "zram",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchDataset {
    Zero,
    Same,
    Repeated,
    Mixed,
    Entropy,
}

impl BenchDataset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Same => "same",
            Self::Repeated => "repeated",
            Self::Mixed => "mixed",
            Self::Entropy => "entropy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaselineSelection {
    None,
    Zstd,
    Lz4,
    Both,
}

impl BaselineSelection {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
            Self::Both => "both",
        }
    }

    fn codecs(self) -> &'static [ExternalCodec] {
        match self {
            Self::None => &[],
            Self::Zstd => &[ExternalCodec::Zstd],
            Self::Lz4 => &[ExternalCodec::Lz4],
            Self::Both => &[ExternalCodec::Zstd, ExternalCodec::Lz4],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalCodec {
    Zstd,
    Lz4,
}

impl ExternalCodec {
    fn as_str(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
        }
    }
}

#[derive(Debug, Clone)]
struct BenchArgs {
    manifest_path: Option<PathBuf>,
    input_path: Option<PathBuf>,
    input_dir: Option<PathBuf>,
    mode: BenchMode,
    dataset: BenchDataset,
    size_bytes: usize,
    iterations: usize,
    warmup: usize,
    page_size: usize,
    min_rle_units: usize,
    dynamic_patterns: bool,
    latency_stats: bool,
    baseline: BaselineSelection,
    export_json: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
struct LatencyStats {
    encode_p50_us: f64,
    encode_p95_us: f64,
    encode_p99_us: f64,
    decode_p50_us: f64,
    decode_p95_us: f64,
    decode_p99_us: f64,
}

#[derive(Debug, Clone, Copy, Default)]
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
    latency: LatencyStats,
}

#[derive(Debug, Clone, Serialize)]
struct BenchReport {
    arch: String,
    requested_mode: String,
    dataset: String,
    input_source: String,
    input_bytes: usize,
    iterations: usize,
    warmup: usize,
    page_size: usize,
    dynamic_patterns: bool,
    latency_stats_flag: bool,
    baseline: String,
    results: Vec<BenchResult>,
}

#[derive(Debug, Clone, Serialize)]
struct BenchResult {
    mode: String,
    codec: String,
    ratio: f64,
    encode_mib_s: f64,
    decode_mib_s: f64,
    encoded_bytes: usize,
    dynamic_patterns: usize,
    dynamic_metadata_bytes: usize,
    encode_ms_total: f64,
    decode_ms_total: f64,
    encode_p50_us: f64,
    encode_p95_us: f64,
    encode_p99_us: f64,
    decode_p50_us: f64,
    decode_p95_us: f64,
    decode_p99_us: f64,
    zram_zero_pages: usize,
    zram_same_pages: usize,
    zram_sxrc_pages: usize,
    zram_raw_pages: usize,
}

impl BenchResult {
    fn from_metrics(mode: &str, codec: &str, metrics: BenchMetrics) -> Self {
        let input_mib = metrics.input_bytes as f64 / (1024.0 * 1024.0);
        let total_mib = input_mib * metrics.iterations as f64;
        Self {
            mode: mode.to_string(),
            codec: codec.to_string(),
            ratio: metrics.encoded_bytes as f64 / metrics.input_bytes as f64,
            encode_mib_s: throughput_mib_s(total_mib, metrics.encode_elapsed),
            decode_mib_s: throughput_mib_s(total_mib, metrics.decode_elapsed),
            encoded_bytes: metrics.encoded_bytes,
            dynamic_patterns: metrics.dynamic_pattern_count,
            dynamic_metadata_bytes: metrics.dynamic_metadata_bytes,
            encode_ms_total: metrics.encode_elapsed.as_secs_f64() * 1000.0,
            decode_ms_total: metrics.decode_elapsed.as_secs_f64() * 1000.0,
            encode_p50_us: metrics.latency.encode_p50_us,
            encode_p95_us: metrics.latency.encode_p95_us,
            encode_p99_us: metrics.latency.encode_p99_us,
            decode_p50_us: metrics.latency.decode_p50_us,
            decode_p95_us: metrics.latency.decode_p95_us,
            decode_p99_us: metrics.latency.decode_p99_us,
            zram_zero_pages: metrics.zram_zero_pages,
            zram_same_pages: metrics.zram_same_pages,
            zram_sxrc_pages: metrics.zram_sxrc_pages,
            zram_raw_pages: metrics.zram_raw_pages,
        }
    }
}

#[derive(Debug, Clone)]
enum ZramPage {
    Zero { original_len: usize },
    Same { value: u8, original_len: usize },
    Data(SxrcCompressedPage),
}

#[derive(Debug, Clone)]
enum ExternalZramPage {
    Zero { original_len: usize },
    Same { value: u8, original_len: usize },
    Data(BaselineCompressedPage),
}

#[derive(Debug, Clone)]
struct BaselineCompressedPage {
    original_len: usize,
    raw: bool,
    encoded: Vec<u8>,
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
    if input.bytes.is_empty() {
        return Err("benchmark input is empty".to_string());
    }

    println!(
        "sxrc-bench arch={} mode={} dataset={} input_source={} input_bytes={} iterations={} warmup={} page_size={} dynamic_patterns={} latency_stats={} baseline={}",
        std::env::consts::ARCH,
        args.mode.as_str(),
        dataset_label(&args, input.source),
        input.source.as_str(),
        input.bytes.len(),
        args.iterations,
        args.warmup,
        config.page_size,
        config.enable_dynamic_patterns,
        args.latency_stats,
        args.baseline.as_str(),
    );

    let mut results = Vec::new();

    if matches!(args.mode, BenchMode::File | BenchMode::Both) {
        let metrics = bench_file_codec(
            &manifest,
            config,
            &input.bytes,
            args.warmup,
            args.iterations,
        )?;
        let result = BenchResult::from_metrics("file", "sxrc", metrics);
        print_metrics(&result);
        results.push(result);

        for codec in args.baseline.codecs() {
            let metrics = bench_file_baseline(*codec, &input.bytes, args.warmup, args.iterations)?;
            let result = BenchResult::from_metrics("file", codec.as_str(), metrics);
            print_metrics(&result);
            results.push(result);
        }
    }

    if matches!(args.mode, BenchMode::Ram | BenchMode::Both) {
        let metrics = bench_ram_codec(
            &manifest,
            config,
            &input.pages,
            &input.bytes,
            args.warmup,
            args.iterations,
        )?;
        let result = BenchResult::from_metrics("ram", "sxrc", metrics);
        print_metrics(&result);
        results.push(result);

        for codec in args.baseline.codecs() {
            let metrics = bench_ram_baseline(
                *codec,
                &input.pages,
                &input.bytes,
                args.warmup,
                args.iterations,
            )?;
            let result = BenchResult::from_metrics("ram", codec.as_str(), metrics);
            print_metrics(&result);
            results.push(result);
        }
    }

    if matches!(args.mode, BenchMode::Zram) {
        let metrics = bench_zram_sim(
            &manifest,
            config,
            &input.pages,
            &input.bytes,
            args.warmup,
            args.iterations,
        )?;
        let result = BenchResult::from_metrics("zram", "sxrc", metrics);
        print_metrics(&result);
        results.push(result);

        for codec in args.baseline.codecs() {
            let metrics = bench_zram_baseline(
                *codec,
                &input.pages,
                &input.bytes,
                args.warmup,
                args.iterations,
            )?;
            let result = BenchResult::from_metrics("zram", codec.as_str(), metrics);
            print_metrics(&result);
            results.push(result);
        }
    }

    if let Some(path) = &args.export_json {
        let report = BenchReport {
            arch: std::env::consts::ARCH.to_string(),
            requested_mode: args.mode.as_str().to_string(),
            dataset: dataset_label(&args, input.source).to_string(),
            input_source: input.source.as_str().to_string(),
            input_bytes: input.bytes.len(),
            iterations: args.iterations,
            warmup: args.warmup,
            page_size: args.page_size,
            dynamic_patterns: args.dynamic_patterns,
            latency_stats_flag: args.latency_stats,
            baseline: args.baseline.as_str().to_string(),
            results,
        };
        let json = serde_json::to_string_pretty(&report)
            .map_err(|err| format!("failed to serialize benchmark JSON: {err}"))?;
        fs::write(path, json)
            .map_err(|err| format!("failed to write benchmark JSON {}: {err}", path.display()))?;
    }

    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<BenchArgs, String> {
    let mut parsed = BenchArgs {
        manifest_path: None,
        input_path: None,
        input_dir: None,
        mode: BenchMode::Both,
        dataset: BenchDataset::Mixed,
        size_bytes: 8 * 1024 * 1024,
        iterations: 16,
        warmup: 0,
        page_size: 4096,
        min_rle_units: 3,
        dynamic_patterns: true,
        latency_stats: false,
        baseline: BaselineSelection::None,
        export_json: None,
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
            "--input-dir" => {
                parsed.input_dir = Some(PathBuf::from(next_arg(&mut args, "--input-dir")?));
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
            "--warmup" => {
                parsed.warmup = parse_usize("--warmup", &next_arg(&mut args, "--warmup")?)?;
            }
            "--page-size" => {
                parsed.page_size =
                    parse_usize("--page-size", &next_arg(&mut args, "--page-size")?)?;
            }
            "--min-rle-units" => {
                parsed.min_rle_units =
                    parse_usize("--min-rle-units", &next_arg(&mut args, "--min-rle-units")?)?;
            }
            "--baseline" => {
                parsed.baseline = match next_arg(&mut args, "--baseline")?.as_str() {
                    "none" => BaselineSelection::None,
                    "zstd" => BaselineSelection::Zstd,
                    "lz4" => BaselineSelection::Lz4,
                    "both" => BaselineSelection::Both,
                    value => {
                        return Err(format!(
                            "unsupported --baseline '{value}', expected none|zstd|lz4|both"
                        ))
                    }
                };
            }
            "--latency-stats" => {
                parsed.latency_stats = true;
            }
            "--export-json" => {
                parsed.export_json = Some(PathBuf::from(next_arg(&mut args, "--export-json")?));
            }
            "--disable-dynamic-patterns" => {
                parsed.dynamic_patterns = false;
            }
            value => return Err(format!("unknown argument '{value}'")),
        }
    }

    if parsed.input_path.is_some() && parsed.input_dir.is_some() {
        return Err("--input and --input-dir are mutually exclusive".to_string());
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
        "Usage: sxrc-bench [--mode file|ram|zram|both] [--dataset zero|same|repeated|mixed|entropy] [--size-bytes N] [--iterations N] [--warmup N] [--page-size N] [--min-rle-units N] [--manifest PATH] [--input PATH | --input-dir DIR] [--baseline none|zstd|lz4|both] [--latency-stats] [--export-json PATH] [--disable-dynamic-patterns]"
    );
}

fn dataset_label(args: &BenchArgs, source: InputSource) -> &'static str {
    match source {
        InputSource::Generated => args.dataset.as_str(),
        InputSource::File | InputSource::Directory => "custom",
    }
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

fn load_or_generate_input(args: &BenchArgs) -> Result<LoadedInput, String> {
    if let Some(path) = &args.input_path {
        let bytes = fs::read(path)
            .map_err(|err| format!("failed to read input {}: {err}", path.display()))?;
        return Ok(LoadedInput {
            pages: bytes
                .chunks(args.page_size)
                .map(|chunk| chunk.to_vec())
                .collect(),
            bytes,
            source: InputSource::File,
        });
    }

    if let Some(path) = &args.input_dir {
        let pages = load_pages_from_dir(path, args.page_size)?;
        return Ok(LoadedInput {
            bytes: flatten_pages(&pages),
            pages,
            source: InputSource::Directory,
        });
    }

    let bytes = match args.dataset {
        BenchDataset::Zero => vec![0_u8; args.size_bytes],
        BenchDataset::Same => vec![0xAA_u8; args.size_bytes],
        BenchDataset::Repeated => generate_repeated_corpus(args.size_bytes),
        BenchDataset::Mixed => generate_mixed_corpus(args.size_bytes),
        BenchDataset::Entropy => generate_entropy_corpus(args.size_bytes),
    };
    Ok(LoadedInput {
        pages: bytes
            .chunks(args.page_size)
            .map(|chunk| chunk.to_vec())
            .collect(),
        bytes,
        source: InputSource::Generated,
    })
}

fn bench_file_codec(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let encoder = SxrcFileEncoder::new(manifest, config).map_err(|err| err.to_string())?;
    let decoder = SxrcFileDecoder::new(manifest, config).map_err(|err| err.to_string())?;
    let total_runs = warmup + iterations;

    let mut encoded_bytes_total = 0usize;
    let mut dynamic_pattern_count_total = 0usize;
    let mut dynamic_metadata_bytes_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations);
    let mut decode_samples = Vec::with_capacity(iterations);

    for index in 0..total_runs {
        let measure = index >= warmup;

        let encode_started = Instant::now();
        let encoded = encoder.encode(input).map_err(|err| err.to_string())?;
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let decoded = decoder
            .decode(&encoded.bytes)
            .map_err(|err| err.to_string())?;
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err("file codec round-trip mismatch".to_string());
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded.bytes.len();
            dynamic_pattern_count_total += encoded.stats.dynamic_pattern_count;
            dynamic_metadata_bytes_total += encoded.stats.dynamic_metadata_bytes;
            encode_samples.push(encode_duration.as_nanos());
            decode_samples.push(decode_duration.as_nanos());
        }

        black_box(encoded.stats);
        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count: dynamic_pattern_count_total / iterations,
        dynamic_metadata_bytes: dynamic_metadata_bytes_total / iterations,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
        ..BenchMetrics::default()
    })
}

fn bench_ram_codec(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    pages: &[Vec<u8>],
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let codec = SxrcRamCodec::new(manifest, config).map_err(|err| err.to_string())?;
    let total_runs = warmup + iterations;

    let mut encoded_bytes_total = 0usize;
    let mut dynamic_pattern_count_total = 0usize;
    let mut dynamic_metadata_bytes_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations * pages.len());
    let mut decode_samples = Vec::with_capacity(iterations * pages.len());

    for index in 0..total_runs {
        let measure = index >= warmup;
        let mut encoded_pages = Vec::with_capacity(pages.len());
        let mut encoded_bytes_iter = 0usize;
        let mut dynamic_pattern_count_iter = 0usize;
        let mut dynamic_metadata_bytes_iter = 0usize;

        let encode_started = Instant::now();
        for page in pages {
            let page_started = Instant::now();
            let compressed = codec.compress_page(page).map_err(|err| err.to_string())?;
            let page_duration = page_started.elapsed();

            if measure {
                encode_samples.push(page_duration.as_nanos());
                encoded_bytes_iter += compressed.encoded.len();
                dynamic_pattern_count_iter += compressed.stats.dynamic_pattern_count;
                dynamic_metadata_bytes_iter += compressed.stats.dynamic_metadata_bytes;
            }

            encoded_pages.push(compressed);
        }
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &encoded_pages {
            let page_started = Instant::now();
            let bytes = codec.decompress_page(page).map_err(|err| err.to_string())?;
            let page_duration = page_started.elapsed();

            if measure {
                decode_samples.push(page_duration.as_nanos());
            }

            decoded.extend_from_slice(&bytes);
        }
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err("RAM codec round-trip mismatch".to_string());
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded_bytes_iter;
            dynamic_pattern_count_total += dynamic_pattern_count_iter;
            dynamic_metadata_bytes_total += dynamic_metadata_bytes_iter;
        }

        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count: dynamic_pattern_count_total / iterations,
        dynamic_metadata_bytes: dynamic_metadata_bytes_total / iterations,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
        ..BenchMetrics::default()
    })
}

fn bench_zram_sim(
    manifest: &SxrcManifest,
    config: SxrcCodecConfig,
    pages: &[Vec<u8>],
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let codec = SxrcRamCodec::new(manifest, config).map_err(|err| err.to_string())?;
    let total_runs = warmup + iterations;

    let mut encoded_bytes_total = 0usize;
    let mut dynamic_pattern_count_total = 0usize;
    let mut dynamic_metadata_bytes_total = 0usize;
    let mut zram_zero_pages_total = 0usize;
    let mut zram_same_pages_total = 0usize;
    let mut zram_sxrc_pages_total = 0usize;
    let mut zram_raw_pages_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations * pages.len());
    let mut decode_samples = Vec::with_capacity(iterations * pages.len());

    for index in 0..total_runs {
        let measure = index >= warmup;
        let mut encoded_pages = Vec::with_capacity(pages.len());
        let mut encoded_bytes_iter = 0usize;
        let mut dynamic_pattern_count_iter = 0usize;
        let mut dynamic_metadata_bytes_iter = 0usize;
        let mut zram_zero_pages_iter = 0usize;
        let mut zram_same_pages_iter = 0usize;
        let mut zram_sxrc_pages_iter = 0usize;
        let mut zram_raw_pages_iter = 0usize;

        let encode_started = Instant::now();
        for page in pages {
            let page_started = Instant::now();
            let encoded_page = if page.iter().all(|byte| *byte == 0) {
                zram_zero_pages_iter += 1;
                ZramPage::Zero {
                    original_len: page.len(),
                }
            } else if let Some(fill_byte) = same_filled_byte(page) {
                zram_same_pages_iter += 1;
                encoded_bytes_iter += 1;
                ZramPage::Same {
                    value: fill_byte,
                    original_len: page.len(),
                }
            } else {
                let compressed = codec.compress_page(page).map_err(|err| err.to_string())?;
                encoded_bytes_iter += compressed.encoded.len();
                dynamic_pattern_count_iter += compressed.stats.dynamic_pattern_count;
                dynamic_metadata_bytes_iter += compressed.stats.dynamic_metadata_bytes;
                match compressed.codec {
                    SxrcPageCodec::Sxrc => zram_sxrc_pages_iter += 1,
                    SxrcPageCodec::Raw => zram_raw_pages_iter += 1,
                }
                ZramPage::Data(compressed)
            };
            let page_duration = page_started.elapsed();

            if measure {
                encode_samples.push(page_duration.as_nanos());
            }

            encoded_pages.push(encoded_page);
        }
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &encoded_pages {
            let page_started = Instant::now();
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
            let page_duration = page_started.elapsed();

            if measure {
                decode_samples.push(page_duration.as_nanos());
            }
        }
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err("ZRAM simulation round-trip mismatch".to_string());
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded_bytes_iter;
            dynamic_pattern_count_total += dynamic_pattern_count_iter;
            dynamic_metadata_bytes_total += dynamic_metadata_bytes_iter;
            zram_zero_pages_total += zram_zero_pages_iter;
            zram_same_pages_total += zram_same_pages_iter;
            zram_sxrc_pages_total += zram_sxrc_pages_iter;
            zram_raw_pages_total += zram_raw_pages_iter;
        }

        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        dynamic_pattern_count: dynamic_pattern_count_total / iterations,
        dynamic_metadata_bytes: dynamic_metadata_bytes_total / iterations,
        zram_zero_pages: zram_zero_pages_total / iterations,
        zram_same_pages: zram_same_pages_total / iterations,
        zram_sxrc_pages: zram_sxrc_pages_total / iterations,
        zram_raw_pages: zram_raw_pages_total / iterations,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
    })
}

fn bench_file_baseline(
    codec: ExternalCodec,
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let total_runs = warmup + iterations;
    let mut encoded_bytes_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations);
    let mut decode_samples = Vec::with_capacity(iterations);

    for index in 0..total_runs {
        let measure = index >= warmup;

        let encode_started = Instant::now();
        let encoded = compress_external(codec, input)?;
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let decoded = decompress_external(codec, &encoded, input.len())?;
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err(format!("{codec:?} file baseline round-trip mismatch"));
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded.len();
            encode_samples.push(encode_duration.as_nanos());
            decode_samples.push(decode_duration.as_nanos());
        }

        black_box(encoded.len());
        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
        ..BenchMetrics::default()
    })
}

fn bench_ram_baseline(
    codec: ExternalCodec,
    pages: &[Vec<u8>],
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let total_runs = warmup + iterations;
    let mut encoded_bytes_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations * pages.len());
    let mut decode_samples = Vec::with_capacity(iterations * pages.len());

    for index in 0..total_runs {
        let measure = index >= warmup;
        let mut encoded_pages = Vec::with_capacity(pages.len());
        let mut encoded_bytes_iter = 0usize;

        let encode_started = Instant::now();
        for page in pages {
            let page_started = Instant::now();
            let compressed = compress_external_page(codec, page, true)?;
            let page_duration = page_started.elapsed();

            if measure {
                encode_samples.push(page_duration.as_nanos());
                encoded_bytes_iter += compressed.encoded.len();
            }

            encoded_pages.push(compressed);
        }
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &encoded_pages {
            let page_started = Instant::now();
            let bytes = decompress_external_page(codec, page)?;
            let page_duration = page_started.elapsed();

            if measure {
                decode_samples.push(page_duration.as_nanos());
            }

            decoded.extend_from_slice(&bytes);
        }
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err(format!("{codec:?} RAM baseline round-trip mismatch"));
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded_bytes_iter;
        }

        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
        ..BenchMetrics::default()
    })
}

fn bench_zram_baseline(
    codec: ExternalCodec,
    pages: &[Vec<u8>],
    input: &[u8],
    warmup: usize,
    iterations: usize,
) -> Result<BenchMetrics, String> {
    let total_runs = warmup + iterations;
    let mut encoded_bytes_total = 0usize;
    let mut zram_zero_pages_total = 0usize;
    let mut zram_same_pages_total = 0usize;
    let mut zram_sxrc_pages_total = 0usize;
    let mut zram_raw_pages_total = 0usize;
    let mut encode_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut encode_samples = Vec::with_capacity(iterations * pages.len());
    let mut decode_samples = Vec::with_capacity(iterations * pages.len());

    for index in 0..total_runs {
        let measure = index >= warmup;
        let mut encoded_pages = Vec::with_capacity(pages.len());
        let mut encoded_bytes_iter = 0usize;
        let mut zram_zero_pages_iter = 0usize;
        let mut zram_same_pages_iter = 0usize;
        let mut zram_sxrc_pages_iter = 0usize;
        let mut zram_raw_pages_iter = 0usize;

        let encode_started = Instant::now();
        for page in pages {
            let page_started = Instant::now();
            let encoded_page = if page.iter().all(|byte| *byte == 0) {
                zram_zero_pages_iter += 1;
                ExternalZramPage::Zero {
                    original_len: page.len(),
                }
            } else if let Some(fill_byte) = same_filled_byte(page) {
                zram_same_pages_iter += 1;
                encoded_bytes_iter += 1;
                ExternalZramPage::Same {
                    value: fill_byte,
                    original_len: page.len(),
                }
            } else {
                let compressed = compress_external_page(codec, page, true)?;
                encoded_bytes_iter += compressed.encoded.len();
                if compressed.raw {
                    zram_raw_pages_iter += 1;
                } else {
                    zram_sxrc_pages_iter += 1;
                }
                ExternalZramPage::Data(compressed)
            };
            let page_duration = page_started.elapsed();

            if measure {
                encode_samples.push(page_duration.as_nanos());
            }

            encoded_pages.push(encoded_page);
        }
        let encode_duration = encode_started.elapsed();

        let decode_started = Instant::now();
        let mut decoded = Vec::with_capacity(input.len());
        for page in &encoded_pages {
            let page_started = Instant::now();
            match page {
                ExternalZramPage::Zero { original_len } => {
                    decoded.resize(decoded.len() + *original_len, 0);
                }
                ExternalZramPage::Same {
                    value,
                    original_len,
                } => {
                    decoded.resize(decoded.len() + *original_len, *value);
                }
                ExternalZramPage::Data(page) => {
                    decoded.extend_from_slice(&decompress_external_page(codec, page)?);
                }
            }
            let page_duration = page_started.elapsed();

            if measure {
                decode_samples.push(page_duration.as_nanos());
            }
        }
        let decode_duration = decode_started.elapsed();

        if decoded != input {
            return Err(format!("{codec:?} zram baseline round-trip mismatch"));
        }

        if measure {
            encode_elapsed += encode_duration;
            decode_elapsed += decode_duration;
            encoded_bytes_total += encoded_bytes_iter;
            zram_zero_pages_total += zram_zero_pages_iter;
            zram_same_pages_total += zram_same_pages_iter;
            zram_sxrc_pages_total += zram_sxrc_pages_iter;
            zram_raw_pages_total += zram_raw_pages_iter;
        }

        black_box(decoded.len());
    }

    Ok(BenchMetrics {
        iterations,
        input_bytes: input.len(),
        encoded_bytes: encoded_bytes_total / iterations,
        encode_elapsed,
        decode_elapsed,
        zram_zero_pages: zram_zero_pages_total / iterations,
        zram_same_pages: zram_same_pages_total / iterations,
        zram_sxrc_pages: zram_sxrc_pages_total / iterations,
        zram_raw_pages: zram_raw_pages_total / iterations,
        latency: build_latency_stats(encode_samples.as_mut_slice(), decode_samples.as_mut_slice()),
        ..BenchMetrics::default()
    })
}

fn compress_external(codec: ExternalCodec, input: &[u8]) -> Result<Vec<u8>, String> {
    match codec {
        ExternalCodec::Zstd => {
            zstd::stream::encode_all(input, 1).map_err(|err| format!("zstd encode failed: {err}"))
        }
        ExternalCodec::Lz4 => Ok(block::compress(input)),
    }
}

fn decompress_external(
    codec: ExternalCodec,
    encoded: &[u8],
    original_len: usize,
) -> Result<Vec<u8>, String> {
    match codec {
        ExternalCodec::Zstd => {
            zstd::stream::decode_all(encoded).map_err(|err| format!("zstd decode failed: {err}"))
        }
        ExternalCodec::Lz4 => block::decompress(encoded, original_len)
            .map_err(|err| format!("lz4 decode failed: {err}")),
    }
}

fn compress_external_page(
    codec: ExternalCodec,
    page: &[u8],
    allow_raw_fallback: bool,
) -> Result<BaselineCompressedPage, String> {
    let encoded = compress_external(codec, page)?;
    if allow_raw_fallback && encoded.len() >= page.len() {
        return Ok(BaselineCompressedPage {
            original_len: page.len(),
            raw: true,
            encoded: page.to_vec(),
        });
    }
    Ok(BaselineCompressedPage {
        original_len: page.len(),
        raw: false,
        encoded,
    })
}

fn decompress_external_page(
    codec: ExternalCodec,
    page: &BaselineCompressedPage,
) -> Result<Vec<u8>, String> {
    if page.raw {
        return Ok(page.encoded.clone());
    }
    decompress_external(codec, &page.encoded, page.original_len)
}

fn print_metrics(result: &BenchResult) {
    println!(
        "mode={} codec={} ratio={:.6} encode_mib_s={:.2} decode_mib_s={:.2} encoded_bytes={} dynamic_patterns={} dynamic_metadata_bytes={} encode_ms_total={:.3} decode_ms_total={:.3} encode_p50_us={:.3} encode_p95_us={:.3} encode_p99_us={:.3} decode_p50_us={:.3} decode_p95_us={:.3} decode_p99_us={:.3} zram_zero_pages={} zram_same_pages={} zram_sxrc_pages={} zram_raw_pages={}",
        result.mode,
        result.codec,
        result.ratio,
        result.encode_mib_s,
        result.decode_mib_s,
        result.encoded_bytes,
        result.dynamic_patterns,
        result.dynamic_metadata_bytes,
        result.encode_ms_total,
        result.decode_ms_total,
        result.encode_p50_us,
        result.encode_p95_us,
        result.encode_p99_us,
        result.decode_p50_us,
        result.decode_p95_us,
        result.decode_p99_us,
        result.zram_zero_pages,
        result.zram_same_pages,
        result.zram_sxrc_pages,
        result.zram_raw_pages,
    );
}

fn build_latency_stats(encode_samples: &mut [u128], decode_samples: &mut [u128]) -> LatencyStats {
    encode_samples.sort_unstable();
    decode_samples.sort_unstable();
    LatencyStats {
        encode_p50_us: percentile_us(encode_samples, 50, 100),
        encode_p95_us: percentile_us(encode_samples, 95, 100),
        encode_p99_us: percentile_us(encode_samples, 99, 100),
        decode_p50_us: percentile_us(decode_samples, 50, 100),
        decode_p95_us: percentile_us(decode_samples, 95, 100),
        decode_p99_us: percentile_us(decode_samples, 99, 100),
    }
}

fn percentile_us(samples: &[u128], numerator: usize, denominator: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let rank = (samples.len() * numerator)
        .div_ceil(denominator)
        .saturating_sub(1);
    samples[rank] as f64 / 1000.0
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
