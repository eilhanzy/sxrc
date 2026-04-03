mod codebook;
mod file_codec;
mod manifest;
mod ram_codec;

pub use file_codec::{
    SxrcCodecConfig, SxrcEncodedPayload, SxrcFileDecoder, SxrcFileEncoder, SxrcStats, SxrcToken,
};
pub use manifest::{
    CompressionUnit, Endian, SxrcDictionaryEntry, SxrcInstructionPattern, SxrcManifest,
};
pub use ram_codec::{SxrcCompressedPage, SxrcPageCodec, SxrcRamCodec};

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SxrcError {
    #[error("manifest parse failed: {0}")]
    ManifestParse(String),
    #[error("duplicate static dictionary id 0x{id:x}")]
    DuplicateDictionaryId { id: u32 },
    #[error("duplicate static dictionary value {value}")]
    DuplicateDictionaryValue { value: String },
    #[error("duplicate instruction pattern id 0x{id:x}")]
    DuplicatePatternId { id: u32 },
    #[error("duplicate instruction pattern bytes {hex_pattern}")]
    DuplicatePatternBytes { hex_pattern: String },
    #[error("invalid hex literal '{literal}'")]
    InvalidHexLiteral { literal: String },
    #[error(
        "dictionary value '{value}' does not fit {unit:?} or does not match the configured width"
    )]
    InvalidDictionaryValue {
        value: String,
        unit: CompressionUnit,
    },
    #[error("memory marker '{name}' has invalid value '{value}'")]
    InvalidMemoryMarker { name: String, value: String },
    #[error("manifest/config mismatch: manifest={manifest:?} config={config:?}")]
    ManifestConfigMismatch {
        manifest: (CompressionUnit, Endian),
        config: (CompressionUnit, Endian),
    },
    #[error("invalid SXRC header")]
    InvalidHeader,
    #[error("unsupported SXRC stream version {version}")]
    UnsupportedVersion { version: u8 },
    #[error("truncated SXRC stream")]
    TruncatedStream,
    #[error("invalid varint in SXRC stream")]
    InvalidVarint,
    #[error("unknown dictionary id 0x{id:x}")]
    UnknownDictionaryId { id: u32 },
    #[error("unknown pattern id 0x{id:x}")]
    UnknownPatternId { id: u32 },
    #[error("invalid RLE run length {count}")]
    InvalidRleRun { count: usize },
    #[error("page length {len} exceeds configured SXRC page size {page_size}")]
    PageTooLarge { len: usize, page_size: usize },
}

pub type Result<T> = std::result::Result<T, SxrcError>;
