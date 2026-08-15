use heatshrink::{Config, encode, decode, EncodeError, DecodeError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HeatshrinkError {
    #[error("Output buffer full during compression")]
    CompressionOutputFull,
    #[error("Output buffer full during decompression")]
    DecompressionOutputFull,
    #[error("Invalid parameters: {0}")]
    InvalidParameters(&'static str),
}

#[derive(Debug, Clone)]
pub struct Heatshrink {
    config: Config,
}

impl Heatshrink {
    pub fn new(window_sz2: u8, lookahead_sz2: u8) -> Result<Self, HeatshrinkError> {
        let config = Config::new(window_sz2, lookahead_sz2)
            .map_err(HeatshrinkError::InvalidParameters)?;
        Ok(Self { config })
    }

    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>, HeatshrinkError> {
        // Safe upper bound estimate for compressed size + some margin
        let mut out = vec![0; data.len() + (data.len() / 2) + 64];
        match encode(data, &mut out, &self.config) {
            Ok(slice) => Ok(slice.to_vec()),
            Err(EncodeError::OutputFull) => Err(HeatshrinkError::CompressionOutputFull),
        }
    }

    pub fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, HeatshrinkError> {
        // Safe upper bound estimate for decompressed size (max 2^lookahead_sz2 ratio, default is usually around 4x-10x)
        // With W=8, L=4 it expands up to roughly 16x in the worst case (theoretically). We use 32x to be safe.
        let mut out = vec![0; data.len() * 32 + 256];
        match decode(data, &mut out, &self.config) {
            Ok(slice) => Ok(slice.to_vec()),
            Err(DecodeError::OutputFull) => Err(HeatshrinkError::DecompressionOutputFull),
        }
    }
}
