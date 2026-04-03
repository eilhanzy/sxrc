use crate::manifest::{CompressionUnit, Endian, SxrcManifest};
use crate::{Result, SxrcError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SxrcCodebook {
    pub unit: CompressionUnit,
    pub endian: Endian,
    pub unit_len: usize,
    dict_id_by_value: BTreeMap<Vec<u8>, u32>,
    dict_value_by_id: BTreeMap<u32, Vec<u8>>,
    pattern_id_by_bytes: BTreeMap<Vec<u8>, u32>,
    pattern_bytes_by_id: BTreeMap<u32, Vec<u8>>,
    ordered_patterns: Vec<(u32, Vec<u8>)>,
}

impl SxrcCodebook {
    pub fn build(manifest: &SxrcManifest, unit: CompressionUnit, endian: Endian) -> Result<Self> {
        if manifest.compression_unit != unit || manifest.endian != endian {
            return Err(SxrcError::ManifestConfigMismatch {
                manifest: (manifest.compression_unit, manifest.endian),
                config: (unit, endian),
            });
        }

        validate_manifest_shape(manifest)?;

        let mut dict_id_by_value = BTreeMap::new();
        let mut dict_value_by_id = BTreeMap::new();
        for entry in &manifest.static_dictionary {
            let value = parse_dictionary_value(&entry.value, unit, endian)?;
            dict_id_by_value.insert(value.clone(), entry.id);
            dict_value_by_id.insert(entry.id, value);
        }

        let mut pattern_id_by_bytes = BTreeMap::new();
        let mut pattern_bytes_by_id = BTreeMap::new();
        let mut ordered_patterns = Vec::new();
        for pattern in &manifest.instruction_patterns {
            let bytes = parse_pattern_bytes(&pattern.hex_pattern)?;
            pattern_id_by_bytes.insert(bytes.clone(), pattern.id);
            pattern_bytes_by_id.insert(pattern.id, bytes.clone());
            ordered_patterns.push((pattern.id, bytes));
        }
        ordered_patterns.sort_by(|left, right| {
            right
                .1
                .len()
                .cmp(&left.1.len())
                .then_with(|| left.0.cmp(&right.0))
        });

        Ok(Self {
            unit,
            endian,
            unit_len: unit.byte_len(),
            dict_id_by_value,
            dict_value_by_id,
            pattern_id_by_bytes,
            pattern_bytes_by_id,
            ordered_patterns,
        })
    }

    pub fn contains_pattern_bytes(&self, bytes: &[u8]) -> bool {
        self.pattern_id_by_bytes.contains_key(bytes)
    }

    pub fn contains_pattern_id(&self, id: u32) -> bool {
        self.pattern_bytes_by_id.contains_key(&id)
    }

    pub fn insert_dynamic_pattern(&mut self, id: u32, bytes: Vec<u8>) -> Result<()> {
        if self.pattern_bytes_by_id.contains_key(&id) {
            return Err(SxrcError::DuplicatePatternId { id });
        }
        if self.pattern_id_by_bytes.contains_key(&bytes) {
            return Err(SxrcError::DuplicatePatternBytes {
                hex_pattern: format_pattern_hex(&bytes),
            });
        }

        self.pattern_id_by_bytes.insert(bytes.clone(), id);
        self.pattern_bytes_by_id.insert(id, bytes.clone());
        self.ordered_patterns.push((id, bytes));
        self.sort_patterns();
        Ok(())
    }

    pub fn dict_id_for_unit(&self, unit: &[u8]) -> Option<u32> {
        self.dict_id_by_value.get(unit).copied()
    }

    pub fn dict_value(&self, id: u32) -> Option<&[u8]> {
        self.dict_value_by_id.get(&id).map(Vec::as_slice)
    }

    pub fn pattern_for_prefix(&self, data: &[u8]) -> Option<(u32, &[u8])> {
        self.ordered_patterns
            .iter()
            .find(|(_, bytes)| data.starts_with(bytes))
            .map(|(id, bytes)| (*id, bytes.as_slice()))
    }

    pub fn pattern_bytes(&self, id: u32) -> Option<&[u8]> {
        self.pattern_bytes_by_id.get(&id).map(Vec::as_slice)
    }
}

fn format_pattern_hex(bytes: &[u8]) -> String {
    let mut output = String::from("0x");
    for byte in bytes {
        output.push_str(&format!("{byte:02X}"));
    }
    output
}

pub(crate) fn validate_manifest_shape(manifest: &SxrcManifest) -> Result<()> {
    let mut dictionary_ids = BTreeSet::new();
    let mut dictionary_values = BTreeSet::new();
    for entry in &manifest.static_dictionary {
        if !dictionary_ids.insert(entry.id) {
            return Err(SxrcError::DuplicateDictionaryId { id: entry.id });
        }
        let value =
            parse_dictionary_value(&entry.value, manifest.compression_unit, manifest.endian)?;
        if !dictionary_values.insert(value.clone()) {
            return Err(SxrcError::DuplicateDictionaryValue {
                value: entry.value.clone(),
            });
        }
    }

    let mut pattern_ids = BTreeSet::new();
    let mut pattern_bytes = BTreeSet::new();
    for pattern in &manifest.instruction_patterns {
        if !pattern_ids.insert(pattern.id) {
            return Err(SxrcError::DuplicatePatternId { id: pattern.id });
        }
        let bytes = parse_pattern_bytes(&pattern.hex_pattern)?;
        if !pattern_bytes.insert(bytes) {
            return Err(SxrcError::DuplicatePatternBytes {
                hex_pattern: pattern.hex_pattern.clone(),
            });
        }
    }

    for (name, value) in &manifest.memory_markers {
        if parse_hex_u64(value).is_err() {
            return Err(SxrcError::InvalidMemoryMarker {
                name: name.clone(),
                value: value.clone(),
            });
        }
    }

    Ok(())
}

fn parse_dictionary_value(value: &str, unit: CompressionUnit, endian: Endian) -> Result<Vec<u8>> {
    let parsed = parse_hex_u64(value)?;
    if parsed > unit.max_value() {
        return Err(SxrcError::InvalidDictionaryValue {
            value: value.to_string(),
            unit,
        });
    }

    let bytes = match (unit, endian) {
        (CompressionUnit::U8, _) => vec![parsed as u8],
        (CompressionUnit::U16, Endian::Little) => (parsed as u16).to_le_bytes().to_vec(),
        (CompressionUnit::U16, Endian::Big) => (parsed as u16).to_be_bytes().to_vec(),
        (CompressionUnit::U32, Endian::Little) => (parsed as u32).to_le_bytes().to_vec(),
        (CompressionUnit::U32, Endian::Big) => (parsed as u32).to_be_bytes().to_vec(),
    };
    Ok(bytes)
}

fn parse_pattern_bytes(pattern: &str) -> Result<Vec<u8>> {
    let hex = normalize_hex(pattern)?;
    if hex.len() % 2 != 0 {
        return Err(SxrcError::InvalidHexLiteral {
            literal: pattern.to_string(),
        });
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for offset in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[offset..offset + 2], 16).map_err(|_| {
            SxrcError::InvalidHexLiteral {
                literal: pattern.to_string(),
            }
        })?;
        bytes.push(byte);
    }
    Ok(bytes)
}

impl SxrcCodebook {
    fn sort_patterns(&mut self) {
        self.ordered_patterns.sort_by(|left, right| {
            right
                .1
                .len()
                .cmp(&left.1.len())
                .then_with(|| left.0.cmp(&right.0))
        });
    }
}

fn parse_hex_u64(value: &str) -> Result<u64> {
    let normalized = normalize_hex(value)?;
    u64::from_str_radix(&normalized, 16).map_err(|_| SxrcError::InvalidHexLiteral {
        literal: value.to_string(),
    })
}

fn normalize_hex(value: &str) -> Result<String> {
    let value = value.trim();
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
        .replace('_', "");

    if hex.is_empty() || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(SxrcError::InvalidHexLiteral {
            literal: value.to_string(),
        });
    }

    Ok(hex)
}
