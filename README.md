# SXRC

SXRC (Super eXtreme RAM Compression) is a dictionary and pattern driven byte
stream codec with a YAML manifest and an optional page-oriented RAM codec.

The crate is designed around two layers:

- `SxrcFileEncoder` / `SxrcFileDecoder` for compact `.sxrc` payloads.
- `SxrcRamCodec` for independent page compression with raw-page fallback.

SXRC is currently optimized for highly repetitive code/data pages, emulator AOT
artifacts, and zram-style userspace experiments. It is not a general-purpose
replacement for entropy coders like zstd on high-entropy data.

## Features

- YAML manifest with static dictionary entries, instruction patterns, and
  memory markers.
- Stream-local dynamic pattern metadata for repeated motifs discovered during
  encoding.
- Literal, dictionary reference, pattern reference, RLE, and raw-escape tokens.
- 8-bit, 16-bit, and 32-bit compression units.
- Little-endian and big-endian dictionary value materialization.
- Page codec with `Raw` fallback when a page is not profitably compressible.
- `sxrc-bench` CLI for file, RAM, and zram simulation benchmarks, baseline
  comparisons, warmups, latency percentiles, and JSON export.
- `sxrc-train` CLI for deriving deterministic static manifests from real page
  corpora.
- Dual license: `MPL-2.0 OR GPL-2.0-or-later`.

## Repository Layout

- [`src/lib.rs`](src/lib.rs): public API and error types.
- [`src/manifest.rs`](src/manifest.rs): YAML manifest model and parser.
- [`src/file_codec.rs`](src/file_codec.rs): `.sxrc` stream encoder/decoder.
- [`src/ram_codec.rs`](src/ram_codec.rs): page codec and raw fallback.
- [`src/bin/sxrc-bench.rs`](src/bin/sxrc-bench.rs): benchmark harness.
- [`src/bin/sxrc-train.rs`](src/bin/sxrc-train.rs): offline corpus trainer for
  static manifests.
- [`docs/format.md`](docs/format.md): binary stream format reference.
- [`docs/manifest.md`](docs/manifest.md): manifest schema and validation rules.
- [`docs/benchmarks.md`](docs/benchmarks.md): benchmark usage and output.
- [`docs/training.md`](docs/training.md): corpus training workflow and trainer
  heuristics.
- [`docs/legacy-linux.md`](docs/legacy-linux.md): real-machine capture and
  reporting workflow for legacy Linux laptops.
- [`docs/zram.md`](docs/zram.md): zram simulation and kernel-port notes.

## Quick Start

### Manifest

```yaml
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"

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

memory_markers:
  stack_init: "0x0A00"
  heap_start: "0x0B00"
```

### File Codec

```rust
use sxrc::{SxrcCodecConfig, SxrcFileDecoder, SxrcFileEncoder, SxrcManifest};

# fn main() -> sxrc::Result<()> {
let manifest = SxrcManifest::from_yaml_str(r#"
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"
static_dictionary:
  - id: 0x0003
    value: "0x07FE"
instruction_patterns:
  - id: 0x0001
    mnemonic: "PUSH_RBP"
    hex_pattern: "0x55"
"#)?;

let config = SxrcCodecConfig::from_manifest(&manifest);
let encoder = SxrcFileEncoder::new(&manifest, config)?;
let decoder = SxrcFileDecoder::new(&manifest, config)?;

let input = [0x07, 0xFE, 0x07, 0xFE, 0x55, 0xAA];
let encoded = encoder.encode(&input)?;
let decoded = decoder.decode(&encoded.bytes)?;

assert_eq!(decoded, input);
println!("ratio={:.6}", encoded.stats.compression_ratio());
# Ok(())
# }
```

### RAM Page Codec

```rust
use sxrc::{SxrcCodecConfig, SxrcManifest, SxrcPageCodec, SxrcRamCodec};

# fn main() -> sxrc::Result<()> {
let manifest = SxrcManifest::from_yaml_str(r#"
version: "1.0-alpha"
target_arch: "generic"
compression_unit: "16-bit"
endian: "little"
static_dictionary:
  - id: 0x0001
    value: "0x0000"
"#)?;

let codec = SxrcRamCodec::new(
    &manifest,
    SxrcCodecConfig {
        page_size: 4096,
        ..SxrcCodecConfig::from_manifest(&manifest)
    },
)?;

let page = vec![0_u8; 4096];
let compressed = codec.compress_page(&page)?;
let restored = codec.decompress_page(&compressed)?;

assert_eq!(restored, page);
assert!(matches!(compressed.codec, SxrcPageCodec::Sxrc | SxrcPageCodec::Raw));
# Ok(())
# }
```

## Benchmarking

Run file/RAM benchmarks on a synthetic mixed workload:

```bash
cargo run --release --bin sxrc-bench -- \
  --mode both \
  --dataset mixed \
  --size-bytes 16777216 \
  --iterations 20 \
  --page-size 4096
```

Run the zram-style simulation:

```bash
cargo run --release --bin sxrc-bench -- \
  --mode zram \
  --dataset repeated \
  --size-bytes 16777216 \
  --iterations 20 \
  --page-size 4096
```

The CLI prints ratio, encode/decode throughput, dynamic metadata size, and
zram page class counts. It can also compare against `zstd`/`lz4`, emit
latency percentiles, and export machine-readable JSON. See
[`docs/benchmarks.md`](docs/benchmarks.md).

Train a manifest from a real page corpus:

```bash
cargo run --release --bin sxrc-train -- \
  --input-dir ./corpus/pages \
  --output ./trained.sxrc.yaml \
  --page-size 4096 \
  --compression-unit 16-bit \
  --endian little \
  --max-dict 64 \
  --max-patterns 32
```

## Format and Manifest Docs

- [`docs/format.md`](docs/format.md) explains the binary header, token layout,
  packed refs, varints, dynamic pattern metadata, and raw fallback behavior.
- [`docs/manifest.md`](docs/manifest.md) documents the YAML schema and all
  validation constraints enforced by the parser/codebook.
- [`docs/training.md`](docs/training.md) covers corpus layout, deterministic
  profile generation, and the trainer's scoring rules.

## Current Limits

- No streaming/incremental decoder API yet. The file codec currently encodes
  and decodes full buffers.
- No checksum/authentication field in the `.sxrc` stream header yet.
- Dynamic patterns are local to each encoded stream/page, not a persistent
  adaptive dictionary shared across payloads.
- `sxrc-bench --mode zram` is a userspace simulation, not a Linux kernel zram
  backend.
- `memory_markers` are validated and preserved in the manifest, but they are
  not consumed by the current encoder/decoder core.

## Testing

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## License

SXRC is dual-licensed under `MPL-2.0 OR GPL-2.0-or-later`.

- [`LICENSE`](LICENSE)
- [`LICENSE.GPL-2.0-or-later`](LICENSE.GPL-2.0-or-later)
- [`LICENSE.DUAL.md`](LICENSE.DUAL.md)
