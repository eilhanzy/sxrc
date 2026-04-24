# SXRC Training

`sxrc-train` derives a deterministic static manifest from a directory of real
page samples.

## Corpus Layout

- Put one page per `*.bin` file.
- Files are read in lexicographic order.
- Each file must be at most `--page-size` bytes.
- Keep corpora outside the repository if they contain sensitive memory data.

Example:

```text
corpus/
  000-page.bin
  001-page.bin
  002-page.bin
```

## Usage

```bash
cargo run --release --bin sxrc-train -- \
  --input-dir ./corpus \
  --output ./trained.sxrc.yaml \
  --page-size 4096 \
  --compression-unit 16-bit \
  --endian little \
  --max-dict 64 \
  --max-patterns 32
```

## What It Learns

- **Static dictionary**: aligned units ranked by frequency-weighted savings.
- **Instruction patterns**: repeated aligned byte motifs up to 16 bytes.
- **Packed-ref priority**: highest-ranked entries get the lowest IDs so IDs
  `0..=63` benefit from one-byte packed refs.

## Determinism Rules

The trainer is designed to emit identical YAML when the input corpus and flags
are identical.

- Dictionary candidates sort by estimated savings, then count, then bytes.
- Pattern candidates sort by estimated savings, then length, then count, then
  bytes.
- IDs are assigned sequentially from `0`.
- Output uses the existing `SxrcManifest` YAML schema.

## Notes

- The trainer currently emits `target_arch: "generic"`.
- Pattern discovery skips unit-RLE motifs that the runtime RLE token already
  handles efficiently.
- Use `sxrc-bench --manifest ./trained.sxrc.yaml --input-dir ./corpus ...` to
  compare the trained profile against the default manifest.
