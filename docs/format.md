# SXRC Stream Format

This document describes the binary payload emitted by `SxrcFileEncoder` and
consumed by `SxrcFileDecoder`.

## High-Level Model

Each encoded payload is one self-contained stream:

1. 8-byte fixed header.
2. Optional stream-local dynamic pattern metadata.
3. Token body.

Static dictionary entries and static instruction patterns are not stored in the
payload body. They come from the YAML manifest used to construct the encoder
and decoder. Dynamic patterns are generated per stream and serialized into the
payload when profitable.

## Header

The header is exactly 8 bytes:

| Offset | Size | Field | Meaning |
| --- | --- | --- | --- |
| 0 | 4 | magic | ASCII `SXRC` |
| 4 | 1 | version | Stream version, currently `1` |
| 5 | 1 | unit width | `1`, `2`, or `4` bytes |
| 6 | 1 | endian | `0` = little-endian, `1` = big-endian |
| 7 | 1 | flags | bit `0x01` = dynamic pattern metadata present |

Decoder checks:

- magic must be `SXRC`
- version must be `1`
- unit width must be `1`, `2`, or `4`
- endian must be `0` or `1`
- unknown flag bits are rejected
- manifest/config unit+endian must match the stream header

## Dynamic Pattern Metadata

When header flag `0x01` is set, dynamic metadata is encoded immediately after
the header:

```text
varint pattern_count
repeat pattern_count times:
  varint pattern_id
  varint pattern_len
  byte[pattern_len] pattern_bytes
```

These patterns are inserted into the decoder's codebook before token decoding
starts. Static and dynamic pattern IDs must not collide.

## Token Body

The token body is a linear stream consumed until EOF.

### Packed Reference Fast Path

IDs `0..=63` use one-byte packed refs:

| Byte Range | Meaning |
| --- | --- |
| `0x40..=0x7F` | dictionary ref, id = byte - `0x40` |
| `0x80..=0xBF` | pattern ref, id = byte - `0x80` |

### Explicit Tokens

| Token | Value | Payload |
| --- | --- | --- |
| `Literal` | `0x00` | `unit_len` raw bytes |
| `DictRef` | `0x01` | `varint dictionary_id` |
| `PatternRef` | `0x02` | `varint pattern_id` |
| `RleRun` | `0x03` | `unit_len` bytes + `varint repeat_count` |
| `RawEscape` | `0x04` | `varint len` + `len` raw bytes |

`repeat_count` must be greater than zero. The decoder raises
`SxrcError::InvalidRleRun` for zero-length runs.

## Varint Encoding

SXRC uses an unsigned base-128 varint:

- lower 7 bits carry payload
- high bit `0x80` means continuation
- decoder accepts at most 10 bytes per varint

Malformed or truncated varints produce `SxrcError::InvalidVarint` or
`SxrcError::TruncatedStream`.

## Encoding Priority

For each offset, the encoder currently tries:

1. RLE over repeated units when count >= `min_rle_units`.
2. Longest static/dynamic instruction pattern match.
3. Static dictionary lookup for one unit.
4. Literal token for one unit.
5. Raw escape for a trailing partial tail smaller than one unit.

Packed refs are emitted when `id <= 63`; otherwise the encoder emits explicit
`DictRef` / `PatternRef` tokens with a varint ID.

## Dynamic Pattern Selection

The encoder scans aligned candidate motifs whose length is a multiple of the
compression unit and at most `max_dynamic_pattern_len`.

A candidate may be selected when:

- it repeats in tiled form at least `min_dynamic_pattern_repeats` times
- it is not already present in the static codebook
- it is not just one repeated unit that RLE already covers
- estimated literal cost is greater than metadata + ref-token cost

Only the best candidates up to `max_dynamic_patterns` are retained, and unused
dynamic patterns are pruned from stream metadata.

## Raw Fallback

If `allow_raw_fallback` is enabled and the compressed body plus dynamic
metadata is larger than a single `RawEscape` token carrying the whole input,
the encoder switches to that raw representation.

For the RAM page codec, `SxrcRamCodec` adds one more guard: if the final SXRC
page payload is not smaller than the source page, the page is stored as
`SxrcPageCodec::Raw` instead.

## Stability Notes

The stream is versioned (`STREAM_VERSION = 1`), but the format is still early
and may evolve before a `1.0` crate release. Treat the current format as an
alpha wire format.
