# SXRC Manifest

SXRC uses a YAML manifest to define the static codebook and architecture
metadata used by the encoder and decoder.

## Schema

```yaml
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"

static_dictionary:
  - id: 0x0001
    value: "0x0000"

instruction_patterns:
  - id: 0x0100
    mnemonic: "MOV_RAX_RBX"
    hex_pattern: "0x4889D8"

memory_markers:
  stack_init: "0x0A00"
  heap_start: "0x0B00"
```

## Fields

### `version`

Free-form manifest version string. The parser stores it, but the codec does not
currently branch on this value.

### `target_arch`

Human-readable architecture label for the manifest profile, for example
`PowerISA-Cell`, `AArch64`, or `generic`.

### `compression_unit`

One of:

- `"8-bit"`
- `"16-bit"`
- `"32-bit"`

This controls the width of dictionary values, literal tokens, and RLE units.

### `endian`

One of:

- `"little"`
- `"big"`

This controls how numeric `static_dictionary.value` literals are materialized
into bytes for the codebook.

### `static_dictionary`

Maps a numeric ID to one fixed-width unit value.

Example:

```yaml
static_dictionary:
  - id: 0x0003
    value: "0x07FE"
```

For a 16-bit big-endian manifest, `0x07FE` becomes bytes `[0x07, 0xFE]`. For a
16-bit little-endian manifest, the same value becomes `[0xFE, 0x07]`.

### `instruction_patterns`

Maps a numeric ID to an arbitrary byte sequence.

Example:

```yaml
instruction_patterns:
  - id: 0x0101
    mnemonic: "PUSH_RBP"
    hex_pattern: "0x55"
```

The `mnemonic` field is descriptive metadata. Matching is done only against
`hex_pattern` bytes.

Patterns are sorted longest-first in the codebook so longer prefixes win before
shorter ones.

### `memory_markers`

Named numeric markers for higher-level tooling:

```yaml
memory_markers:
  stack_init: "0x0A00"
  heap_start: "0x0B00"
```

The current codec validates and preserves these markers but does not consume
them during tokenization.

## Validation Rules

`SxrcManifest::from_yaml_str` parses YAML and then validates all structural
constraints:

- duplicate static dictionary IDs are rejected
- duplicate static dictionary values are rejected after endian-aware
  materialization
- duplicate instruction pattern IDs are rejected
- duplicate instruction pattern byte sequences are rejected
- static dictionary values must fit the selected `compression_unit`
- `hex_pattern` and marker values must be valid hex literals
- marker values may include `0x` / `0X` prefixes and `_` separators

When building a codec, `SxrcFileEncoder::new`, `SxrcFileDecoder::new`, and
`SxrcRamCodec::new` also require `config.compression_unit` and `config.endian`
to match the manifest. Otherwise they return
`SxrcError::ManifestConfigMismatch`.

## Minimal Manifest

A manifest may omit `static_dictionary`, `instruction_patterns`, and
`memory_markers`. In that case SXRC still works with literals, RLE,
raw escapes, and stream-local dynamic patterns:

```yaml
version: "1.0-alpha"
target_arch: "generic"
compression_unit: "16-bit"
endian: "little"
```

## Recommendations

- Use low numeric IDs (`0..=63`) for hot dictionary/pattern entries so they can
  be encoded as one-byte packed refs.
- Put very common one-unit values in `static_dictionary`.
- Put longer opcode/data motifs in `instruction_patterns`.
- Keep `compression_unit` aligned with the data you expect to compress. For
  example, `16-bit` is a good default for repeated halfword-heavy emulator
  data, while `8-bit` may be better for byte-oriented streams.
- Prefer architecture-specific manifests for known workloads, and use dynamic
  patterns to catch per-stream repetition that was not known ahead of time.
