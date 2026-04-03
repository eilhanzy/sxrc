use crate::{
    Result, SxrcCodecConfig, SxrcEncodedPayload, SxrcError, SxrcFileDecoder, SxrcFileEncoder,
    SxrcManifest, SxrcStats,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SxrcCompressedPage {
    pub original_len: usize,
    pub codec: SxrcPageCodec,
    pub encoded: Vec<u8>,
    pub stats: SxrcStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SxrcPageCodec {
    Sxrc,
    Raw,
}

#[derive(Debug, Clone)]
pub struct SxrcRamCodec {
    encoder: SxrcFileEncoder,
    decoder: SxrcFileDecoder,
    page_size: usize,
}

impl SxrcRamCodec {
    pub fn new(manifest: &SxrcManifest, config: SxrcCodecConfig) -> Result<Self> {
        Ok(Self {
            encoder: SxrcFileEncoder::new(manifest, config)?,
            decoder: SxrcFileDecoder::new(manifest, config)?,
            page_size: config.page_size,
        })
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn compress_page(&self, page: &[u8]) -> Result<SxrcCompressedPage> {
        if page.len() > self.page_size {
            return Err(SxrcError::PageTooLarge {
                len: page.len(),
                page_size: self.page_size,
            });
        }

        let SxrcEncodedPayload { bytes, stats } = self.encoder.encode(page)?;
        if bytes.len() >= page.len() {
            return Ok(SxrcCompressedPage {
                original_len: page.len(),
                codec: SxrcPageCodec::Raw,
                encoded: page.to_vec(),
                stats: SxrcStats {
                    raw_bytes: page.len(),
                    encoded_bytes: page.len(),
                    ..SxrcStats::default()
                },
            });
        }

        Ok(SxrcCompressedPage {
            original_len: page.len(),
            codec: SxrcPageCodec::Sxrc,
            encoded: bytes,
            stats,
        })
    }

    pub fn decompress_page(&self, page: &SxrcCompressedPage) -> Result<Vec<u8>> {
        let decoded = match page.codec {
            SxrcPageCodec::Sxrc => self.decoder.decode(&page.encoded)?,
            SxrcPageCodec::Raw => page.encoded.clone(),
        };
        Ok(decoded)
    }
}
