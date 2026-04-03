use crate::{codebook::validate_manifest_shape, Result, SxrcError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionUnit {
    #[serde(rename = "8-bit")]
    U8,
    #[serde(rename = "16-bit")]
    U16,
    #[serde(rename = "32-bit")]
    U32,
}

impl CompressionUnit {
    pub fn byte_len(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U32 => 4,
        }
    }

    pub fn max_value(self) -> u64 {
        match self {
            Self::U8 => u8::MAX as u64,
            Self::U16 => u16::MAX as u64,
            Self::U32 => u32::MAX as u64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SxrcDictionaryEntry {
    pub id: u32,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SxrcInstructionPattern {
    pub id: u32,
    pub mnemonic: String,
    pub hex_pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SxrcManifest {
    pub version: String,
    pub target_arch: String,
    pub compression_unit: CompressionUnit,
    pub endian: Endian,
    #[serde(default)]
    pub static_dictionary: Vec<SxrcDictionaryEntry>,
    #[serde(default)]
    pub instruction_patterns: Vec<SxrcInstructionPattern>,
    #[serde(default)]
    pub memory_markers: BTreeMap<String, String>,
}

impl SxrcManifest {
    pub fn from_yaml_str(input: &str) -> Result<Self> {
        let manifest =
            serde_yaml::from_str(input).map_err(|err| SxrcError::ManifestParse(err.to_string()))?;
        validate_manifest_shape(&manifest)?;
        Ok(manifest)
    }

    pub fn to_yaml_string(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|err| SxrcError::ManifestParse(err.to_string()))
    }

    pub fn validate(&self) -> Result<()> {
        validate_manifest_shape(self)
    }
}
