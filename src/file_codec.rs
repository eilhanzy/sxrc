use crate::codebook::SxrcCodebook;
use crate::manifest::{CompressionUnit, Endian, SxrcManifest};
use crate::{Result, SxrcError};
use std::collections::{BTreeMap, BTreeSet};

const MAGIC: &[u8; 4] = b"SXRC";
const STREAM_VERSION: u8 = 1;
const HEADER_FLAG_DYNAMIC_PATTERNS: u8 = 0x01;
const TOKEN_LITERAL: u8 = 0x00;
const TOKEN_DICT_REF: u8 = 0x01;
const TOKEN_PATTERN_REF: u8 = 0x02;
const TOKEN_RLE_RUN: u8 = 0x03;
const TOKEN_RAW_ESCAPE: u8 = 0x04;
const PACKED_DICT_BASE: u8 = 0x40;
const PACKED_PATTERN_BASE: u8 = 0x80;
const PACKED_REF_MAX: u32 = 0x3f;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SxrcCodecConfig {
    pub compression_unit: CompressionUnit,
    pub endian: Endian,
    pub min_rle_units: usize,
    pub page_size: usize,
    pub allow_raw_fallback: bool,
    pub enable_dynamic_patterns: bool,
    pub min_dynamic_pattern_repeats: usize,
    pub max_dynamic_pattern_len: usize,
    pub max_dynamic_patterns: usize,
}

impl Default for SxrcCodecConfig {
    fn default() -> Self {
        Self {
            compression_unit: CompressionUnit::U16,
            endian: Endian::Big,
            min_rle_units: 3,
            page_size: 4096,
            allow_raw_fallback: true,
            enable_dynamic_patterns: true,
            min_dynamic_pattern_repeats: 3,
            max_dynamic_pattern_len: 16,
            max_dynamic_patterns: 32,
        }
    }
}

impl SxrcCodecConfig {
    pub fn from_manifest(manifest: &SxrcManifest) -> Self {
        Self {
            compression_unit: manifest.compression_unit,
            endian: manifest.endian,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SxrcStats {
    pub raw_bytes: usize,
    pub encoded_bytes: usize,
    pub token_count: usize,
    pub literal_tokens: usize,
    pub dict_tokens: usize,
    pub pattern_tokens: usize,
    pub rle_tokens: usize,
    pub raw_escape_tokens: usize,
    pub dynamic_pattern_count: usize,
    pub dynamic_metadata_bytes: usize,
}

impl SxrcStats {
    pub fn compression_ratio(self) -> f64 {
        if self.raw_bytes == 0 {
            return 1.0;
        }
        self.encoded_bytes as f64 / self.raw_bytes as f64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SxrcEncodedPayload {
    pub bytes: Vec<u8>,
    pub stats: SxrcStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SxrcToken {
    Literal,
    DictRef,
    PatternRef,
    RleRun,
    RawEscape,
}

#[derive(Debug, Clone)]
pub struct SxrcFileEncoder {
    config: SxrcCodecConfig,
    codebook: SxrcCodebook,
}

impl SxrcFileEncoder {
    pub fn new(manifest: &SxrcManifest, config: SxrcCodecConfig) -> Result<Self> {
        Ok(Self {
            codebook: SxrcCodebook::build(manifest, config.compression_unit, config.endian)?,
            config,
        })
    }

    pub fn encode(&self, input: &[u8]) -> Result<SxrcEncodedPayload> {
        let mut dynamic_patterns = self.build_dynamic_patterns(input)?;
        let mut codebook = self.codebook.clone();
        for pattern in &dynamic_patterns {
            codebook.insert_dynamic_pattern(pattern.id, pattern.bytes.clone())?;
        }

        let mut body = Vec::new();
        let mut stats = SxrcStats {
            raw_bytes: input.len(),
            ..SxrcStats::default()
        };
        let mut used_pattern_ids = BTreeSet::new();
        let mut offset = 0usize;

        while offset < input.len() {
            if offset + codebook.unit_len <= input.len() {
                let unit = &input[offset..offset + codebook.unit_len];
                let rle_count = self.count_repeated_units(&codebook, input, offset, unit);
                if rle_count >= self.config.min_rle_units {
                    body.push(TOKEN_RLE_RUN);
                    body.extend_from_slice(unit);
                    write_varint(rle_count as u64, &mut body);
                    stats.rle_tokens += 1;
                    stats.token_count += 1;
                    offset += codebook.unit_len * rle_count;
                    continue;
                }
            }

            if let Some((pattern_id, pattern_bytes)) = codebook.pattern_for_prefix(&input[offset..])
            {
                let token_len = pattern_ref_len(pattern_id);
                if token_len <= pattern_bytes.len() {
                    write_pattern_ref(pattern_id, &mut body);
                    used_pattern_ids.insert(pattern_id);
                    stats.pattern_tokens += 1;
                    stats.token_count += 1;
                    offset += pattern_bytes.len();
                    continue;
                }
            }

            if offset + codebook.unit_len <= input.len() {
                let unit = &input[offset..offset + codebook.unit_len];
                if let Some(dict_id) = codebook.dict_id_for_unit(unit) {
                    let token_len = dict_ref_len(dict_id);
                    if token_len <= codebook.unit_len {
                        write_dict_ref(dict_id, &mut body);
                        stats.dict_tokens += 1;
                        stats.token_count += 1;
                        offset += codebook.unit_len;
                        continue;
                    }
                }

                body.push(TOKEN_LITERAL);
                body.extend_from_slice(unit);
                stats.literal_tokens += 1;
                stats.token_count += 1;
                offset += codebook.unit_len;
                continue;
            }

            let tail = &input[offset..];
            body.push(TOKEN_RAW_ESCAPE);
            write_varint(tail.len() as u64, &mut body);
            body.extend_from_slice(tail);
            stats.raw_escape_tokens += 1;
            stats.token_count += 1;
            offset = input.len();
        }

        dynamic_patterns.retain(|pattern| used_pattern_ids.contains(&pattern.id));
        let dynamic_metadata = encode_dynamic_pattern_metadata(&dynamic_patterns);
        stats.dynamic_pattern_count = dynamic_patterns.len();
        stats.dynamic_metadata_bytes = dynamic_metadata.len();

        if self.config.allow_raw_fallback && !input.is_empty() {
            let mut raw_body = Vec::with_capacity(1 + varint_len(input.len() as u64) + input.len());
            raw_body.push(TOKEN_RAW_ESCAPE);
            write_varint(input.len() as u64, &mut raw_body);
            raw_body.extend_from_slice(input);

            if raw_body.len() < body.len() + dynamic_metadata.len() {
                body = raw_body;
                stats = SxrcStats {
                    raw_bytes: input.len(),
                    token_count: 1,
                    raw_escape_tokens: 1,
                    ..SxrcStats::default()
                };
            }
        }

        let has_dynamic_patterns = !dynamic_metadata.is_empty() && stats.dynamic_pattern_count > 0;
        let mut bytes = Vec::with_capacity(8 + dynamic_metadata.len() + body.len());
        write_header(&self.config, has_dynamic_patterns, &mut bytes);
        if has_dynamic_patterns {
            bytes.extend_from_slice(&dynamic_metadata);
        }
        bytes.extend_from_slice(&body);
        stats.encoded_bytes = bytes.len();
        Ok(SxrcEncodedPayload { bytes, stats })
    }

    fn count_repeated_units(
        &self,
        codebook: &SxrcCodebook,
        input: &[u8],
        offset: usize,
        unit: &[u8],
    ) -> usize {
        let mut count = 1usize;
        let mut cursor = offset + codebook.unit_len;
        while cursor + codebook.unit_len <= input.len()
            && &input[cursor..cursor + codebook.unit_len] == unit
        {
            count += 1;
            cursor += codebook.unit_len;
        }
        count
    }

    fn build_dynamic_patterns(&self, input: &[u8]) -> Result<Vec<DynamicPatternEntry>> {
        if !self.config.enable_dynamic_patterns || input.is_empty() {
            return Ok(Vec::new());
        }

        let mut occurrences: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
        for pattern_len in dynamic_pattern_lengths(self.codebook.unit_len, input.len(), self.config)
        {
            for offset in (0..=input.len() - pattern_len).step_by(self.codebook.unit_len) {
                let pattern = input[offset..offset + pattern_len].to_vec();
                if self.codebook.contains_pattern_bytes(&pattern) {
                    continue;
                }
                occurrences.entry(pattern).or_default().push(offset);
            }
        }

        let mut candidates = occurrences
            .into_iter()
            .filter_map(|(bytes, offsets)| {
                let repeats = tiled_repeat_count(&offsets, bytes.len());
                if repeats < self.config.min_dynamic_pattern_repeats
                    || is_unit_rle_pattern(&bytes, self.codebook.unit_len)
                {
                    return None;
                }
                Some((bytes, repeats))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let left_score =
                dynamic_pattern_base_cost(left.0.len(), left.1, self.codebook.unit_len);
            let right_score =
                dynamic_pattern_base_cost(right.0.len(), right.1, self.codebook.unit_len);
            right_score
                .cmp(&left_score)
                .then_with(|| right.0.len().cmp(&left.0.len()))
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut selected = Vec::new();
        let mut used_ids = BTreeSet::new();
        let mut next_id = 0u32;

        for (bytes, repeats) in candidates {
            if selected.len() >= self.config.max_dynamic_patterns {
                break;
            }
            if selected
                .iter()
                .any(|pattern: &DynamicPatternEntry| pattern.bytes == bytes)
            {
                continue;
            }

            next_id = first_free_pattern_id(next_id, &self.codebook, &used_ids);
            let metadata_cost =
                varint_len(next_id as u64) + varint_len(bytes.len() as u64) + bytes.len();
            let token_cost = repeats * pattern_ref_len(next_id);
            let base_cost = dynamic_pattern_base_cost(bytes.len(), repeats, self.codebook.unit_len);

            if base_cost > token_cost + metadata_cost {
                used_ids.insert(next_id);
                selected.push(DynamicPatternEntry { id: next_id, bytes });
                next_id = next_id.saturating_add(1);
            }
        }

        Ok(selected)
    }
}

#[derive(Debug, Clone)]
pub struct SxrcFileDecoder {
    config: SxrcCodecConfig,
    codebook: SxrcCodebook,
}

impl SxrcFileDecoder {
    pub fn new(manifest: &SxrcManifest, config: SxrcCodecConfig) -> Result<Self> {
        Ok(Self {
            codebook: SxrcCodebook::build(manifest, config.compression_unit, config.endian)?,
            config,
        })
    }

    pub fn decode(&self, encoded: &[u8]) -> Result<Vec<u8>> {
        let header = verify_header(encoded, self.config)?;
        let mut cursor = header.cursor;
        let mut codebook = self.codebook.clone();
        if header.has_dynamic_patterns {
            for pattern in decode_dynamic_pattern_metadata(encoded, &mut cursor)? {
                codebook.insert_dynamic_pattern(pattern.id, pattern.bytes)?;
            }
        }
        let mut output = Vec::new();

        while cursor < encoded.len() {
            let token = encoded[cursor];
            cursor += 1;

            match token {
                PACKED_DICT_BASE..=0x7f => {
                    let id = u32::from(token - PACKED_DICT_BASE);
                    let unit = codebook
                        .dict_value(id)
                        .ok_or(SxrcError::UnknownDictionaryId { id })?;
                    output.extend_from_slice(unit);
                }
                PACKED_PATTERN_BASE..=0xbf => {
                    let id = u32::from(token - PACKED_PATTERN_BASE);
                    let bytes = codebook
                        .pattern_bytes(id)
                        .ok_or(SxrcError::UnknownPatternId { id })?;
                    output.extend_from_slice(bytes);
                }
                TOKEN_LITERAL => {
                    let unit = read_bytes(encoded, &mut cursor, codebook.unit_len)?;
                    output.extend_from_slice(unit);
                }
                TOKEN_DICT_REF => {
                    let id = read_varint(encoded, &mut cursor)? as u32;
                    let unit = codebook
                        .dict_value(id)
                        .ok_or(SxrcError::UnknownDictionaryId { id })?;
                    output.extend_from_slice(unit);
                }
                TOKEN_PATTERN_REF => {
                    let id = read_varint(encoded, &mut cursor)? as u32;
                    let bytes = codebook
                        .pattern_bytes(id)
                        .ok_or(SxrcError::UnknownPatternId { id })?;
                    output.extend_from_slice(bytes);
                }
                TOKEN_RLE_RUN => {
                    let unit = read_bytes(encoded, &mut cursor, codebook.unit_len)?;
                    let count = read_varint(encoded, &mut cursor)? as usize;
                    if count == 0 {
                        return Err(SxrcError::InvalidRleRun { count });
                    }
                    for _ in 0..count {
                        output.extend_from_slice(unit);
                    }
                }
                TOKEN_RAW_ESCAPE => {
                    let len = read_varint(encoded, &mut cursor)? as usize;
                    let bytes = read_bytes(encoded, &mut cursor, len)?;
                    output.extend_from_slice(bytes);
                }
                _ => return Err(SxrcError::InvalidHeader),
            }
        }

        Ok(output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DynamicPatternEntry {
    id: u32,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SxrcStreamHeader {
    cursor: usize,
    has_dynamic_patterns: bool,
}

fn write_header(config: &SxrcCodecConfig, has_dynamic_patterns: bool, output: &mut Vec<u8>) {
    output.extend_from_slice(MAGIC);
    output.push(STREAM_VERSION);
    output.push(match config.compression_unit {
        CompressionUnit::U8 => 1,
        CompressionUnit::U16 => 2,
        CompressionUnit::U32 => 4,
    });
    output.push(match config.endian {
        Endian::Little => 0,
        Endian::Big => 1,
    });
    output.push(if has_dynamic_patterns {
        HEADER_FLAG_DYNAMIC_PATTERNS
    } else {
        0
    });
}

fn verify_header(input: &[u8], config: SxrcCodecConfig) -> Result<SxrcStreamHeader> {
    if input.len() < 8 || &input[..4] != MAGIC {
        return Err(SxrcError::InvalidHeader);
    }
    if input[4] != STREAM_VERSION {
        return Err(SxrcError::UnsupportedVersion { version: input[4] });
    }
    let unit = match input[5] {
        1 => CompressionUnit::U8,
        2 => CompressionUnit::U16,
        4 => CompressionUnit::U32,
        _ => return Err(SxrcError::InvalidHeader),
    };
    let endian = match input[6] {
        0 => Endian::Little,
        1 => Endian::Big,
        _ => return Err(SxrcError::InvalidHeader),
    };
    if config.compression_unit != unit || config.endian != endian {
        return Err(SxrcError::ManifestConfigMismatch {
            manifest: (unit, endian),
            config: (config.compression_unit, config.endian),
        });
    }
    let flags = input[7];
    if flags & !HEADER_FLAG_DYNAMIC_PATTERNS != 0 {
        return Err(SxrcError::InvalidHeader);
    }
    Ok(SxrcStreamHeader {
        cursor: 8,
        has_dynamic_patterns: flags & HEADER_FLAG_DYNAMIC_PATTERNS != 0,
    })
}

fn dynamic_pattern_lengths(
    unit_len: usize,
    input_len: usize,
    config: SxrcCodecConfig,
) -> Vec<usize> {
    let max_len = config.max_dynamic_pattern_len.min(input_len);
    let mut lengths = (unit_len..=max_len)
        .filter(|len| len.is_multiple_of(unit_len))
        .collect::<Vec<_>>();
    lengths.sort_unstable_by(|left, right| right.cmp(left));
    lengths
}

fn dynamic_pattern_base_cost(pattern_len: usize, repeats: usize, unit_len: usize) -> usize {
    let literal_units = pattern_len / unit_len;
    repeats * literal_units * (unit_len + 1)
}

fn is_unit_rle_pattern(bytes: &[u8], unit_len: usize) -> bool {
    if bytes.len() < unit_len || !bytes.len().is_multiple_of(unit_len) {
        return false;
    }

    let first = &bytes[..unit_len];
    bytes.chunks_exact(unit_len).all(|chunk| chunk == first)
}

fn tiled_repeat_count(offsets: &[usize], pattern_len: usize) -> usize {
    let mut best = 0usize;
    for (start_index, start_offset) in offsets.iter().enumerate() {
        let mut count = 1usize;
        let mut next_offset = start_offset + pattern_len;
        for offset in &offsets[start_index + 1..] {
            if *offset == next_offset {
                count += 1;
                next_offset += pattern_len;
            } else if *offset > next_offset {
                break;
            }
        }
        best = best.max(count);
    }
    best
}

fn first_free_pattern_id(
    mut next_id: u32,
    codebook: &SxrcCodebook,
    used_ids: &BTreeSet<u32>,
) -> u32 {
    while codebook.contains_pattern_id(next_id) || used_ids.contains(&next_id) {
        next_id = next_id.saturating_add(1);
    }
    next_id
}

fn encode_dynamic_pattern_metadata(patterns: &[DynamicPatternEntry]) -> Vec<u8> {
    if patterns.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::new();
    write_varint(patterns.len() as u64, &mut output);
    for pattern in patterns {
        write_varint(pattern.id as u64, &mut output);
        write_varint(pattern.bytes.len() as u64, &mut output);
        output.extend_from_slice(&pattern.bytes);
    }
    output
}

fn decode_dynamic_pattern_metadata(
    input: &[u8],
    cursor: &mut usize,
) -> Result<Vec<DynamicPatternEntry>> {
    let count = read_varint(input, cursor)? as usize;
    let mut patterns = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_varint(input, cursor)? as u32;
        let len = read_varint(input, cursor)? as usize;
        let bytes = read_bytes(input, cursor, len)?.to_vec();
        patterns.push(DynamicPatternEntry { id, bytes });
    }
    Ok(patterns)
}

fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn write_dict_ref(id: u32, output: &mut Vec<u8>) {
    if id <= PACKED_REF_MAX {
        output.push(PACKED_DICT_BASE + id as u8);
        return;
    }
    output.push(TOKEN_DICT_REF);
    write_varint(id as u64, output);
}

fn write_pattern_ref(id: u32, output: &mut Vec<u8>) {
    if id <= PACKED_REF_MAX {
        output.push(PACKED_PATTERN_BASE + id as u8);
        return;
    }
    output.push(TOKEN_PATTERN_REF);
    write_varint(id as u64, output);
}

fn dict_ref_len(id: u32) -> usize {
    if id <= PACKED_REF_MAX {
        1
    } else {
        1 + varint_len(id as u64)
    }
}

fn pattern_ref_len(id: u32) -> usize {
    if id <= PACKED_REF_MAX {
        1
    } else {
        1 + varint_len(id as u64)
    }
}

fn read_varint(input: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;

    for _ in 0..10 {
        if *cursor >= input.len() {
            return Err(SxrcError::TruncatedStream);
        }
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }

    Err(SxrcError::InvalidVarint)
}

fn read_bytes<'a>(input: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = cursor.checked_add(len).ok_or(SxrcError::TruncatedStream)?;
    if end > input.len() {
        return Err(SxrcError::TruncatedStream);
    }
    let bytes = &input[*cursor..end];
    *cursor = end;
    Ok(bytes)
}

fn varint_len(mut value: u64) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        len += 1;
        value >>= 7;
    }
    len
}
