use sxrc::{
    CompressionUnit, Endian, SxrcCodecConfig, SxrcError, SxrcFileDecoder, SxrcFileEncoder,
    SxrcManifest, SxrcPageCodec, SxrcRamCodec,
};

fn sample_manifest_yaml() -> &'static str {
    r#"
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
"#
}

#[test]
fn file_codec_roundtrips_dictionary_patterns_rle_and_tail_bytes() {
    let manifest = SxrcManifest::from_yaml_str(sample_manifest_yaml()).unwrap();
    let config = SxrcCodecConfig {
        min_rle_units: 3,
        enable_dynamic_patterns: false,
        ..SxrcCodecConfig::from_manifest(&manifest)
    };
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let mut input = Vec::new();
    for _ in 0..8 {
        input.extend_from_slice(&[
            0x07, 0xFE, 0x07, 0xFE, 0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90, 0x48, 0x89, 0xD8, 0x55,
        ]);
    }
    input.push(0xAB);

    let encoded = encoder.encode(&input).unwrap();
    let decoded = decoder.decode(&encoded.bytes).unwrap();

    assert_eq!(decoded, input);
    assert!(encoded.stats.compression_ratio() < 1.0);
    assert!(encoded.stats.rle_tokens >= 1);
    assert!(encoded.stats.dict_tokens >= 1);
    assert!(encoded.stats.pattern_tokens >= 1);
    assert!(encoded.stats.raw_escape_tokens >= 1);
}

#[test]
fn file_codec_embeds_profitable_dynamic_patterns_in_stream_metadata() {
    let yaml = r#"
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"
"#;
    let manifest = SxrcManifest::from_yaml_str(yaml).unwrap();
    let config = SxrcCodecConfig {
        enable_dynamic_patterns: true,
        min_dynamic_pattern_repeats: 3,
        max_dynamic_pattern_len: 8,
        max_dynamic_patterns: 8,
        ..SxrcCodecConfig::from_manifest(&manifest)
    };
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let mut input = Vec::new();
    for _ in 0..6 {
        input.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0xCA, 0xFE]);
    }
    input.extend_from_slice(&[0x44, 0x55]);

    let encoded = encoder.encode(&input).unwrap();
    let decoded = decoder.decode(&encoded.bytes).unwrap();

    assert_eq!(decoded, input);
    assert!(encoded.stats.dynamic_pattern_count >= 1);
    assert!(encoded.stats.dynamic_metadata_bytes > 0);
    assert!(encoded.stats.pattern_tokens >= 1);
    assert!(encoded.stats.compression_ratio() < 1.0);
}

#[test]
fn little_endian_and_32_bit_units_roundtrip() {
    let yaml = r#"
version: "1.0-alpha"
target_arch: "AArch64"
compression_unit: "32-bit"
endian: "little"
static_dictionary:
  - id: 0x0001
    value: "0x11223344"
"#;
    let manifest = SxrcManifest::from_yaml_str(yaml).unwrap();
    let config = SxrcCodecConfig::from_manifest(&manifest);
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let input = [0x44, 0x33, 0x22, 0x11, 0x44, 0x33, 0x22, 0x11];
    let encoded = encoder.encode(&input).unwrap();
    let decoded = decoder.decode(&encoded.bytes).unwrap();

    assert_eq!(decoded, input);
    assert_eq!(manifest.compression_unit, CompressionUnit::U32);
    assert_eq!(manifest.endian, Endian::Little);
}

#[test]
fn eight_bit_units_roundtrip() {
    let yaml = r#"
version: "1.0-alpha"
target_arch: "generic"
compression_unit: "8-bit"
endian: "big"
static_dictionary:
  - id: 0x0001
    value: "0xAA"
"#;
    let manifest = SxrcManifest::from_yaml_str(yaml).unwrap();
    let config = SxrcCodecConfig::from_manifest(&manifest);
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let input = [0xAA, 0xAA, 0xAA, 0x01];
    let encoded = encoder.encode(&input).unwrap();
    let decoded = decoder.decode(&encoded.bytes).unwrap();

    assert_eq!(decoded, input);
    assert_eq!(manifest.compression_unit, CompressionUnit::U8);
}

#[test]
fn manifest_rejects_duplicate_ids_and_invalid_markers() {
    let duplicate_dict = r#"
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"
static_dictionary:
  - id: 0x0001
    value: "0x0000"
  - id: 0x0001
    value: "0xFFFF"
"#;
    assert_eq!(
        SxrcManifest::from_yaml_str(duplicate_dict).unwrap_err(),
        SxrcError::DuplicateDictionaryId { id: 1 }
    );

    let invalid_marker = r#"
version: "1.0-alpha"
target_arch: "PowerISA-Cell"
compression_unit: "16-bit"
endian: "big"
memory_markers:
  stack_init: "not-hex"
"#;
    assert_eq!(
        SxrcManifest::from_yaml_str(invalid_marker).unwrap_err(),
        SxrcError::InvalidMemoryMarker {
            name: "stack_init".to_string(),
            value: "not-hex".to_string(),
        }
    );
}

#[test]
fn incompressible_payload_uses_raw_fallback_and_decoder_rejects_truncation() {
    let manifest = SxrcManifest::from_yaml_str(sample_manifest_yaml()).unwrap();
    let config = SxrcCodecConfig::from_manifest(&manifest);
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let input = [0x10, 0x20, 0x30, 0x40, 0x50];
    let encoded = encoder.encode(&input).unwrap();

    assert_eq!(decoder.decode(&encoded.bytes).unwrap(), input);
    assert_eq!(encoded.stats.raw_escape_tokens, 1);

    let truncated = &encoded.bytes[..encoded.bytes.len() - 1];
    assert_eq!(
        decoder.decode(truncated).unwrap_err(),
        SxrcError::TruncatedStream
    );
}

#[test]
fn ram_codec_compresses_independent_pages_and_enforces_page_size() {
    let manifest = SxrcManifest::from_yaml_str(sample_manifest_yaml()).unwrap();
    let config = SxrcCodecConfig {
        page_size: 32,
        ..SxrcCodecConfig::from_manifest(&manifest)
    };
    let codec = SxrcRamCodec::new(&manifest, config).unwrap();

    let mut input_a = Vec::new();
    for _ in 0..15 {
        input_a.extend_from_slice(&[0x07, 0xFE]);
    }
    input_a.extend_from_slice(&[0x04, 0xEE]);

    let mut input_b = Vec::new();
    for _ in 0..12 {
        input_b.extend_from_slice(&[0x90, 0x90]);
    }
    input_b.extend_from_slice(&[0x48, 0x89, 0xD8, 0x55, 0xAA, 0xBB, 0xCC, 0xDD]);

    let page_a = codec.compress_page(&input_a).unwrap();
    let page_b = codec.compress_page(&input_b).unwrap();

    assert_eq!(page_a.codec, SxrcPageCodec::Sxrc);
    assert_eq!(codec.decompress_page(&page_a).unwrap(), input_a);
    assert_eq!(codec.decompress_page(&page_b).unwrap(), input_b);
    assert!(page_a.stats.compression_ratio() <= 1.5);

    assert_eq!(
        codec.compress_page(&[0u8; 33]).unwrap_err(),
        SxrcError::PageTooLarge {
            len: 33,
            page_size: 32,
        }
    );
}

#[test]
fn ram_codec_uses_raw_fallback_for_incompressible_pages() {
    let manifest = SxrcManifest::from_yaml_str(sample_manifest_yaml()).unwrap();
    let codec = SxrcRamCodec::new(
        &manifest,
        SxrcCodecConfig {
            page_size: 64,
            ..SxrcCodecConfig::from_manifest(&manifest)
        },
    )
    .unwrap();

    let input = (0u8..64).collect::<Vec<_>>();
    let page = codec.compress_page(&input).unwrap();

    assert_eq!(page.codec, SxrcPageCodec::Raw);
    assert_eq!(page.encoded, input);
    assert_eq!(page.stats.compression_ratio(), 1.0);
    assert_eq!(codec.decompress_page(&page).unwrap(), input);
}

#[test]
fn page_local_dynamic_patterns_require_meaningful_net_savings() {
    let manifest = SxrcManifest::from_yaml_str(
        r#"
version: "1.0-alpha"
target_arch: "generic"
compression_unit: "16-bit"
endian: "big"
"#,
    )
    .unwrap();
    let config = SxrcCodecConfig {
        page_size: 64,
        enable_dynamic_patterns: true,
        min_dynamic_pattern_repeats: 3,
        max_dynamic_pattern_len: 4,
        max_dynamic_patterns: 8,
        ..SxrcCodecConfig::from_manifest(&manifest)
    };
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let input = [
        0xAA, 0xBB, 0xCC, 0xDD, 0xAA, 0xBB, 0xCC, 0xDD, 0xAA, 0xBB, 0xCC, 0xDD,
    ];
    let encoded = encoder.encode(&input).unwrap();

    assert_eq!(decoder.decode(&encoded.bytes).unwrap(), input);
    assert_eq!(encoded.stats.dynamic_pattern_count, 0);
    assert_eq!(encoded.stats.dynamic_metadata_bytes, 0);
}

#[test]
fn sxrc_stats_can_be_compared_with_zstd_ratio() {
    let manifest = SxrcManifest::from_yaml_str(sample_manifest_yaml()).unwrap();
    let config = SxrcCodecConfig::from_manifest(&manifest);
    let encoder = SxrcFileEncoder::new(&manifest, config).unwrap();
    let decoder = SxrcFileDecoder::new(&manifest, config).unwrap();

    let mut input = Vec::new();
    for _ in 0..64 {
        input.extend_from_slice(&[0x07, 0xFE, 0x07, 0xFE, 0x04, 0xEE, 0x90, 0x90]);
        input.extend_from_slice(&[0x48, 0x89, 0xD8, 0x55]);
    }

    let sxrc_encoded = encoder.encode(&input).unwrap();
    let zstd_encoded = zstd::stream::encode_all(input.as_slice(), 1).unwrap();

    assert_eq!(decoder.decode(&sxrc_encoded.bytes).unwrap(), input);
    assert!(sxrc_encoded.stats.encoded_bytes < sxrc_encoded.stats.raw_bytes);
    assert!(!zstd_encoded.is_empty());
}
