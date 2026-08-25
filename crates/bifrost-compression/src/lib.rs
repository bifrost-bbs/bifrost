use heatshrink::{decode, encode, Config, DecodeError, EncodeError};
use std::collections::HashMap;
use thiserror::Error;

/// Error types occurring during compression or decompression.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompressionError {
    #[error("Output buffer full during compression")]
    CompressionOutputFull,
    #[error("Output buffer full during decompression")]
    DecompressionOutputFull,
    #[error("Invalid parameters: {0}")]
    InvalidParameters(&'static str),
    #[error("Corrupt dictionary format: {0}")]
    CorruptDictionary(&'static str),
    #[error("Invalid token reference 0x{0:02X}")]
    InvalidTokenReference(u8),
    #[error("CRC32 mismatch on dictionary: expected 0x{expected:08X}, got 0x{actual:08X}")]
    DictionaryCrcMismatch { expected: u32, actual: u32 },
}

pub type HeatshrinkError = CompressionError;

/// Standard IEEE 802.3 CRC32 calculation.
pub fn calc_crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (!((crc & 1) != 0) as u32).wrapping_add(1);
            crc = (crc >> 1) ^ (0xEDB88320 & mask);
        }
    }
    !crc
}

/// A compact domain dictionary for tokenizing common byte strings, ANSI opcodes, and UI phrases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionDictionary {
    tokens: Vec<Vec<u8>>,
    crc32: u32,
}

const DICT_MAGIC: &[u8; 4] = b"BFDC";
const DICT_VERSION: u8 = 1;
const TOKEN_ESCAPE: u8 = 0xFD;
const ESCAPED_LITERAL: u8 = 0xFF;

impl CompressionDictionary {
    /// Creates a new CompressionDictionary with validated tokens (max 254 tokens, each 2..=64 bytes).
    pub fn new(tokens: Vec<Vec<u8>>) -> Result<Self, CompressionError> {
        if tokens.len() > 254 {
            return Err(CompressionError::InvalidParameters(
                "dictionary cannot contain more than 254 tokens",
            ));
        }
        let mut clean_tokens = Vec::new();
        for t in tokens {
            if t.len() >= 2 && t.len() <= 64 && !clean_tokens.contains(&t) {
                clean_tokens.push(t);
            }
        }
        let mut dict = Self {
            tokens: clean_tokens,
            crc32: 0,
        };
        dict.crc32 = calc_crc32(&dict.serialize_payload());
        Ok(dict)
    }

    /// Returns the active tokens in this dictionary.
    pub fn tokens(&self) -> &[Vec<u8>] {
        &self.tokens
    }

    /// Returns the CRC32 checksum of this dictionary.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    fn serialize_payload(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(DICT_MAGIC);
        buf.push(DICT_VERSION);
        buf.push(self.tokens.len() as u8);
        for t in &self.tokens {
            buf.push(t.len() as u8);
            buf.extend_from_slice(t);
        }
        buf
    }

    /// Serializes dictionary to binary format with header and CRC32.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut payload = self.serialize_payload();
        payload.extend_from_slice(&self.crc32.to_be_bytes());
        payload
    }

    /// Deserializes dictionary from binary format, verifying magic, version, and CRC32.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CompressionError> {
        if bytes.len() < 10 {
            return Err(CompressionError::CorruptDictionary(
                "dictionary header too short",
            ));
        }
        if &bytes[0..4] != DICT_MAGIC {
            return Err(CompressionError::CorruptDictionary("invalid magic bytes"));
        }
        if bytes[4] != DICT_VERSION {
            return Err(CompressionError::CorruptDictionary(
                "unsupported dictionary version",
            ));
        }
        let count = bytes[5] as usize;
        let mut offset = 6;
        let mut tokens = Vec::with_capacity(count);

        for _ in 0..count {
            if offset >= bytes.len() - 4 {
                return Err(CompressionError::CorruptDictionary(
                    "unexpected EOF reading tokens",
                ));
            }
            let t_len = bytes[offset] as usize;
            offset += 1;
            if offset + t_len > bytes.len() - 4 {
                return Err(CompressionError::CorruptDictionary(
                    "token length out of bounds",
                ));
            }
            tokens.push(bytes[offset..offset + t_len].to_vec());
            offset += t_len;
        }

        if offset != bytes.len() - 4 {
            return Err(CompressionError::CorruptDictionary(
                "trailing payload before CRC",
            ));
        }

        let expected_crc = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let actual_crc = calc_crc32(&bytes[..offset]);
        if expected_crc != actual_crc {
            return Err(CompressionError::DictionaryCrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        Ok(Self {
            tokens,
            crc32: expected_crc,
        })
    }

    /// Encodes input using dictionary tokenization.
    pub fn compress(&self, input: &[u8]) -> Vec<u8> {
        if self.tokens.is_empty() || input.is_empty() {
            return input.to_vec();
        }

        let mut output = Vec::with_capacity(input.len());
        let mut i = 0;

        while i < input.len() {
            // Check for matching tokens (prioritizing longer matches)
            let mut best_match: Option<(usize, usize)> = None; // (token_idx, token_len)

            for (t_idx, token) in self.tokens.iter().enumerate() {
                let t_len = token.len();
                if i + t_len <= input.len() && &input[i..i + t_len] == token.as_slice() {
                    match best_match {
                        Some((_, best_len)) if t_len > best_len => {
                            best_match = Some((t_idx, t_len));
                        }
                        None => {
                            best_match = Some((t_idx, t_len));
                        }
                        _ => {}
                    }
                }
            }

            if let Some((t_idx, t_len)) = best_match {
                output.push(TOKEN_ESCAPE);
                output.push(t_idx as u8);
                i += t_len;
            } else {
                let byte = input[i];
                if byte == TOKEN_ESCAPE {
                    output.push(TOKEN_ESCAPE);
                    output.push(ESCAPED_LITERAL);
                } else {
                    output.push(byte);
                }
                i += 1;
            }
        }

        output
    }

    /// Decodes dictionary tokenized stream back to original bytes.
    pub fn decompress(&self, input: &[u8]) -> Result<Vec<u8>, CompressionError> {
        let mut output = Vec::with_capacity(input.len() * 2);
        let mut i = 0;

        while i < input.len() {
            let byte = input[i];
            if byte == TOKEN_ESCAPE {
                if i + 1 >= input.len() {
                    return Err(CompressionError::CorruptDictionary(
                        "truncated escape sequence",
                    ));
                }
                let next_byte = input[i + 1];
                if next_byte == ESCAPED_LITERAL {
                    output.push(TOKEN_ESCAPE);
                } else if (next_byte as usize) < self.tokens.len() {
                    output.extend_from_slice(&self.tokens[next_byte as usize]);
                } else {
                    log::warn!(
                        "[DECOMPRESS] Unknown token index 0x{:02X} (dictionary size: {}). Inserting space fallback.",
                        next_byte,
                        self.tokens.len()
                    );
                    output.push(b' ');
                }
                i += 2;
            } else {
                output.push(byte);
                i += 1;
            }
        }

        Ok(output)
    }

    /// Standard static dictionary containing common ANSI commands, box drawing, and BBS phrases.
    pub fn standard_static() -> Self {
        let raw_tokens: &[&[u8]] = &[
            // ANSI Attributes & Control Sequences
            &[0xC0, 0x07],             // Set Color Light Gray
            &[0xC0, 0x0F],             // Set Color Bright White
            &[0xC0, 0x0E],             // Set Color Yellow
            &[0xC0, 0x0A],             // Set Color Bright Green
            &[0xC0, 0x09],             // Set Color Bright Blue
            &[0xC0, 0x0C],             // Set Color Bright Red
            &[0xC0, 0x0D],             // Set Color Bright Magenta
            &[0xC0, 0x0B],             // Set Color Bright Cyan
            &[0x01, 0xC3, 0x01, 0x01], // Clear screen + cursor to 1,1
            &[0xD0, 0x01],             // Form Start
            &[0xD3, 0x04],             // Form End + EndOfFrame
            &[0xC5, 0x01],             // Render asset prefix
            b"----------------------------------------",
            b"========================================",
            b"[Submit]",
            b"[Cancel]",
            b"Welcome to ",
            b"Press Enter to continue",
            b"Press Enter",
            b"Select an option",
            b"Enter your ",
            b"Logged in as: ",
            b"Nickname",
            b"Password",
            b"Messages",
            b"Marketplace",
            b"Dungeon",
            b"Profile",
            b"Admin",
            b"Main Menu",
            b"Level: ",
            b"Gold: ",
            b"Health: ",
            b"Inventory",
            b"Commands:",
            b"[Y/n]",
            b"(y/n)",
            b"Description",
            b"Price:",
            b"Seller:",
            b"Subject:",
            b"From: ",
            b"Date: ",
            b"Node ID: ",
            b"  ",
            b"    ",
        ];

        let tokens = raw_tokens.iter().map(|s| s.to_vec()).collect();
        Self::new(tokens).expect("Standard static dictionary must be valid")
    }
}

/// Extracts frequent N-grams from sample packet payloads to train optimal domain dictionaries.
pub struct DictionaryTrainer;

impl DictionaryTrainer {
    /// Trains a custom dictionary from sample packet payloads, preserving base standard tokens.
    pub fn train_from_samples(samples: &[&[u8]], max_tokens: usize) -> CompressionDictionary {
        let max_tokens = std::cmp::min(254, max_tokens);
        let mut ngram_freq: HashMap<Vec<u8>, usize> = HashMap::new();

        // 1. Scan for candidate N-grams (lengths 3 to 24)
        for sample in samples {
            if sample.len() < 3 {
                continue;
            }
            for len in 3..=std::cmp::min(24, sample.len()) {
                for window in sample.windows(len) {
                    *ngram_freq.entry(window.to_vec()).or_insert(0) += 1;
                }
            }
        }

        // 2. Score candidates by net byte savings: (count - 1) * (len - 2)
        let mut scored_candidates: Vec<(Vec<u8>, isize)> = ngram_freq
            .into_iter()
            .filter_map(|(ngram, count)| {
                if count >= 2 {
                    let score = (count as isize - 1) * (ngram.len() as isize - 2);
                    if score > 0 {
                        Some((ngram, score))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Sort descending by score
        scored_candidates.sort_by(|a, b| b.1.cmp(&a.1));

        // 3. Start with base standard static tokens, then greedily append top candidates
        let mut selected_tokens = CompressionDictionary::standard_static().tokens;
        for (candidate, _) in scored_candidates {
            if selected_tokens.len() >= max_tokens {
                break;
            }
            // Add token if not duplicate
            if !selected_tokens.contains(&candidate) {
                selected_tokens.push(candidate);
            }
        }

        CompressionDictionary::new(selected_tokens)
            .unwrap_or_else(|_| CompressionDictionary::standard_static())
    }
}

/// Low-level Heatshrink LZSS wrapper.
#[derive(Debug, Clone)]
pub struct Heatshrink {
    config: Config,
}

impl Heatshrink {
    pub fn new(window_sz2: u8, lookahead_sz2: u8) -> Result<Self, CompressionError> {
        if !(4..=14).contains(&window_sz2) {
            return Err(CompressionError::InvalidParameters(
                "window size must be between 4 and 14",
            ));
        }
        if lookahead_sz2 < 3 || lookahead_sz2 > window_sz2 {
            return Err(CompressionError::InvalidParameters(
                "lookahead size must be between 3 and window size",
            ));
        }
        let config =
            Config::new(window_sz2, lookahead_sz2).map_err(CompressionError::InvalidParameters)?;
        Ok(Self { config })
    }

    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>, CompressionError> {
        let in_bytes = data.len();
        let mut out = vec![0; data.len() + (data.len() / 2) + 64];
        match encode(data, &mut out, &self.config) {
            Ok(slice) => {
                let out_bytes = slice.len();
                let savings_pct = if in_bytes > 0 {
                    ((in_bytes as f64 - out_bytes as f64) / in_bytes as f64) * 100.0
                } else {
                    0.0
                };
                log::trace!(
                    "[HEATSHRINK COMPRESS] Input: {} B, Output: {} B, Savings: {:.1}%",
                    in_bytes,
                    out_bytes,
                    savings_pct
                );
                Ok(slice.to_vec())
            }
            Err(EncodeError::OutputFull) => Err(CompressionError::CompressionOutputFull),
        }
    }

    pub fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, CompressionError> {
        let in_bytes = data.len();
        let mut out = vec![0; data.len() * 32 + 256];
        match decode(data, &mut out, &self.config) {
            Ok(slice) => {
                let out_bytes = slice.len();
                log::trace!(
                    "[HEATSHRINK DECOMPRESS] Input: {} B, Output: {} B",
                    in_bytes,
                    out_bytes
                );
                Ok(slice.to_vec())
            }
            Err(DecodeError::OutputFull) => Err(CompressionError::DecompressionOutputFull),
        }
    }
}

/// Compresses payload using an adaptive multi-tier pipeline (Dict, LZSS, Dict+LZSS, or Raw fallback).
/// Returns `(flags, payload)` where flags indicate the compression mode used:
/// - `0x00`: Raw / Uncompressed (used whenever compression expands payload)
/// - `0x02`: Heatshrink LZSS only
/// - `0x04`: Dictionary tokenization only
/// - `0x06`: Dictionary tokenization + Heatshrink LZSS
pub fn compress_adaptive(
    data: &[u8],
    dict: Option<&CompressionDictionary>,
    window_sz2: u8,
    lookahead_sz2: u8,
) -> (u8, Vec<u8>) {
    if data.is_empty() {
        return (0x00, Vec::new());
    }

    let raw_len = data.len();
    let mut best_flags = 0x00u8;
    let mut best_payload = data.to_vec();
    let mut best_len = raw_len;

    let hs_opt = Heatshrink::new(window_sz2, lookahead_sz2).ok();

    // 1. Evaluate LZSS only
    if let Some(ref hs) = hs_opt {
        if let Ok(comp) = hs.compress(data) {
            if comp.len() < best_len {
                best_len = comp.len();
                best_flags = 0x02;
                best_payload = comp;
            }
        }
    }

    // 2. Evaluate Dictionary only
    let dict_encoded = dict.map(|d| d.compress(data));
    if let Some(ref dict_data) = dict_encoded {
        if dict_data.len() < best_len {
            best_len = dict_data.len();
            best_flags = 0x04;
            best_payload = dict_data.clone();
        }

        // 3. Evaluate Dictionary + LZSS
        if let Some(ref hs) = hs_opt {
            if let Ok(combo) = hs.compress(dict_data) {
                if combo.len() < best_len {
                    best_flags = 0x06;
                    best_payload = combo;
                }
            }
        }
    }

    let (algo_name, savings_pct) = match best_flags & 0x06 {
        0x02 => (
            "heatshrink_w8_l4",
            ((raw_len as f64 - best_payload.len() as f64) / raw_len as f64) * 100.0,
        ),
        0x04 => (
            "domain_dict",
            ((raw_len as f64 - best_payload.len() as f64) / raw_len as f64) * 100.0,
        ),
        0x06 => (
            "domain_dict+heatshrink",
            ((raw_len as f64 - best_payload.len() as f64) / raw_len as f64) * 100.0,
        ),
        _ => ("raw_fallback (0% expansion)", 0.0),
    };
    log::debug!(
        "[ADAPTIVE COMPRESS] Chosen: {} | Raw: {} B -> Out: {} B ({:+.1}% savings)",
        algo_name,
        raw_len,
        best_payload.len(),
        savings_pct
    );

    (best_flags, best_payload)
}

/// Decompresses payload based on flags and optional dictionary.
pub fn decompress_adaptive(
    flags: u8,
    data: &[u8],
    dict: Option<&CompressionDictionary>,
    window_sz2: u8,
    lookahead_sz2: u8,
) -> Result<Vec<u8>, CompressionError> {
    match flags & 0x06 {
        0x00 => Ok(data.to_vec()),
        0x02 => {
            let hs = Heatshrink::new(window_sz2, lookahead_sz2)?;
            hs.decompress(data)
        }
        0x04 => {
            let d = dict.ok_or(CompressionError::CorruptDictionary(
                "dictionary required but not provided",
            ))?;
            d.decompress(data)
        }
        0x06 => {
            let hs = Heatshrink::new(window_sz2, lookahead_sz2)?;
            let lzss_decomp = hs.decompress(data)?;
            let d = dict.ok_or(CompressionError::CorruptDictionary(
                "dictionary required but not provided",
            ))?;
            d.decompress(&lzss_decomp)
        }
        _ => Ok(data.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heatshrink_compress_decompress_roundtrip() {
        let hs = Heatshrink::new(8, 4).expect("Config 8, 4 should be valid");
        let original =
            b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps again!";
        let compressed = hs.compress(original).expect("Compression should succeed");
        let decompressed = hs
            .decompress(&compressed)
            .expect("Decompression should succeed");
        assert_eq!(original.to_vec(), decompressed);
    }

    #[test]
    fn test_heatshrink_repetitive_data_compression() {
        let hs = Heatshrink::new(8, 4).unwrap();
        let original = vec![0xAB; 256];
        let compressed = hs.compress(&original).unwrap();
        assert!(
            compressed.len() < original.len(),
            "Repetitive data should compress significantly"
        );
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
        let hs_err = Heatshrink::new(4, 5);
        assert!(hs_err.is_err(), "lookahead > window should fail");
    }

    #[test]
    fn test_dictionary_encode_decode_roundtrip() {
        let dict = CompressionDictionary::standard_static();
        let original = b"Welcome to Bifrost! Select an option from the Main Menu. Nickname: User1";
        let compressed = dict.compress(original);
        assert!(
            compressed.len() < original.len(),
            "Dictionary compression should reduce size for known phrases"
        );
        let decompressed = dict
            .decompress(&compressed)
            .expect("Decompression should succeed");
        assert_eq!(original.to_vec(), decompressed);
    }

    #[test]
    fn test_dictionary_with_escaped_literal() {
        let dict = CompressionDictionary::standard_static();
        let original = vec![0xFD, 0x01, 0xFD, 0xFF, 0xFD];
        let compressed = dict.compress(&original);
        let decompressed = dict.decompress(&compressed).unwrap();
        assert_eq!(original, decompressed);
    }

    #[test]
    fn test_dictionary_serialization_roundtrip() {
        let dict = CompressionDictionary::standard_static();
        let bytes = dict.to_bytes();
        let restored =
            CompressionDictionary::from_bytes(&bytes).expect("Deserialization should succeed");
        assert_eq!(dict.tokens, restored.tokens);
        assert_eq!(dict.crc32, restored.crc32);
    }

    #[test]
    fn test_dictionary_trainer() {
        let samples: Vec<&[u8]> = vec![
            b"Welcome to the Grand Dragon Arena. Select an option: (1) Attack (2) Defend",
            b"Welcome to the Grand Dragon Arena. Select an option: (1) Attack (3) Inventory",
            b"Welcome to the Grand Dragon Arena. Select an option: (4) Retreat",
        ];

        let trained = DictionaryTrainer::train_from_samples(&samples, 10);
        assert!(!trained.tokens().is_empty());
        let test_str = b"Welcome to the Grand Dragon Arena. Select an option: (1) Attack";
        let comp = trained.compress(test_str);
        assert!(
            comp.len() < test_str.len(),
            "Trained dictionary should compress test string"
        );
        let decomp = trained.decompress(&comp).unwrap();
        assert_eq!(test_str.to_vec(), decomp);
    }

    #[test]
    fn test_adaptive_compression_never_expands() {
        let dict = CompressionDictionary::standard_static();
        // High entropy data that usually expands in LZSS
        let random_bytes = vec![0x12, 0x98, 0x34, 0x76, 0x55, 0xFA, 0x09, 0x33, 0x88, 0x19];
        let (flags, payload) = compress_adaptive(&random_bytes, Some(&dict), 8, 4);
        assert!(
            payload.len() <= random_bytes.len(),
            "Adaptive compression must never exceed raw length"
        );
        let decomp = decompress_adaptive(flags, &payload, Some(&dict), 8, 4).unwrap();
        assert_eq!(random_bytes, decomp);
    }

    #[test]
    fn test_compression_error_display() {
        assert_eq!(
            CompressionError::CompressionOutputFull.to_string(),
            "Output buffer full during compression"
        );
        assert_eq!(
            CompressionError::DecompressionOutputFull.to_string(),
            "Output buffer full during decompression"
        );
        assert_eq!(
            CompressionError::InvalidParameters("bad window").to_string(),
            "Invalid parameters: bad window"
        );
        assert_eq!(
            CompressionError::CorruptDictionary("bad magic").to_string(),
            "Corrupt dictionary format: bad magic"
        );
        assert_eq!(
            CompressionError::InvalidTokenReference(0x99).to_string(),
            "Invalid token reference 0x99"
        );
    }
}
