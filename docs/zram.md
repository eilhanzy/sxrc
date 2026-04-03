# SXRC and zram

SXRC currently includes a userspace zram-style benchmark mode, but it is not a
Linux kernel zram compressor module yet.

## What `sxrc-bench --mode zram` Does

The benchmark simulates a simple zram page pipeline:

1. Split input into fixed-size pages.
2. If a page is all zeroes, store it as a zero page with 0 payload bytes.
3. Else if a page is filled with one repeated byte, store that single fill
   byte.
4. Else call `SxrcRamCodec::compress_page(page)`.
5. If SXRC does not beat raw size, the RAM codec stores the page as
   `SxrcPageCodec::Raw`.
6. Decode the simulated page list and verify byte-for-byte equality.

Metrics printed by the CLI include page counts for each class and total ratio.

## Why This Is Not a Kernel Backend Yet

The current Rust crate depends on userspace-friendly pieces (`serde`,
`serde_yaml`, standard library collections, heap-owned `Vec<u8>` buffers) and
is structured around manifest parsing plus whole-buffer APIs.

A real Linux zram backend would need a dedicated kernel-side implementation
with different constraints:

- no YAML parsing on the hot path
- no unrestricted heap allocation patterns in compression/decompression paths
- no `std` dependency
- bounded per-page metadata
- careful locking and per-CPU workspace handling
- compatibility with the kernel compression API expected by zram/zsmalloc

## Practical Porting Strategy

A realistic path is to split SXRC into two layers:

- **Userspace profile tooling**: keep YAML manifests, offline dictionary
  generation, benchmark tooling, and corpus analysis in this crate.
- **Kernel codec core**: implement a small fixed-profile compressor/decompressor
  in C or `no_std` Rust style, fed by a generated static dictionary table.

## Candidate Kernel-Side Profile

For a first zram prototype, avoid dynamic stream metadata and start with a
bounded static profile:

- one fixed `compression_unit` (likely 16-bit)
- one endianness profile per target
- a small static dictionary with low packed IDs
- optional short instruction patterns
- RLE and raw fallback
- page-local encoding only

That keeps decode simple and makes worst-case memory overhead easier to reason
about.

## Suggested Next Milestones

- Generate a compact static C header from an SXRC YAML manifest.
- Write a tiny page compressor/decompressor prototype independent of `serde`.
- Add a differential test that compares the kernel-profile codec against this
  Rust implementation on the same corpus.
- Measure ratio and latency against zstd/lz4-style baselines on repeated,
  mixed, and entropy-heavy page sets.
- Only then wire the prototype into a real zram compressor module.

## Current Development Guidance

Use `sxrc-bench --mode zram` to answer workload questions early, especially:

- How many pages become `Zero`, `Same`, `Sxrc`, or `Raw`?
- Does your dictionary help repeated emulator pages enough to beat raw?
- Does dynamic metadata help or hurt at 4 KiB page granularity?
- What is the decode speed tradeoff on AArch64, PowerISA64, and x86_64 hosts?

For kernel work, treat this userspace mode as a model and validation harness,
not as the final in-kernel implementation.
