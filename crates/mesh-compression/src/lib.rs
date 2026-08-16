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
        if !(4..=14).contains(&window_sz2) {
            return Err(HeatshrinkError::InvalidParameters("window size must be between 4 and 14"));
        }
        if lookahead_sz2 < 3 || lookahead_sz2 > window_sz2 {
            return Err(HeatshrinkError::InvalidParameters("lookahead size must be between 3 and window size"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heatshrink_compress_decompress_roundtrip() {
        let hs = Heatshrink::new(8, 4).expect("Config 8, 4 should be valid");
        let original = b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps again!";
        let compressed = hs.compress(original).expect("Compression should succeed");
        let decompressed = hs.decompress(&compressed).expect("Decompression should succeed");
        assert_eq!(original.to_vec(), decompressed);
    }

    #[test]
    fn test_heatshrink_repetitive_data_compression() {
        let hs = Heatshrink::new(8, 4).unwrap();
        let original = vec![0xAB; 256];
        let compressed = hs.compress(&original).unwrap();
        assert!(compressed.len() < original.len(), "Repetitive data should compress significantly");
        let decompressed = hs.decompress(&compressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_heatshrink_empty_data() {
        let hs = Heatshrink::new(8, 4).unwrap();
        let original = b"";
        let compressed = hs.compress(original).unwrap();
        let decompressed = hs.decompress(&compressed).unwrap();
        assert_eq!(original.to_vec(), decompressed);
    }

    #[test]
    fn test_heatshrink_invalid_parameters() {
        // lookahead > window or window out of range
        let hs_err = Heatshrink::new(4, 5);
        assert!(hs_err.is_err(), "lookahead > window should fail");
    }

    #[test]
    fn test_heatshrink_error_display() {
        assert_eq!(
            HeatshrinkError::CompressionOutputFull.to_string(),
            "Output buffer full during compression"
        );
        assert_eq!(
            HeatshrinkError::DecompressionOutputFull.to_string(),
            "Output buffer full during decompression"
        );
        assert_eq!(
            HeatshrinkError::InvalidParameters("bad window").to_string(),
            "Invalid parameters: bad window"
        );
    }
}

