//! MeshANSI compiler and compression library.
//! Handles converting ANSI sequences into 1-byte opcodes, differential drawing,
//! and Heatshrink LZSS compression.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MeshAnsiError {
    #[error("Failed to parse ANSI sequence: {0}")]
    ParseError(String),
    #[error("Compression error: {0}")]
    CompressionError(String),
    #[error("Decompression error: {0}")]
    DecompressionError(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// MeshANSI Bytecode Opcode definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    Nop = 0x00,
    ClearScreen = 0x01,
    Crlf = 0x02,
    PagePause = 0x03,
    EndOfFrame = 0x04,
    SetColor = 0xC0,
    RleGlyph = 0xC1,
    RleSpace = 0xC2,
    CursorAbs = 0xC3,
    CursorRel = 0xC4,
    RenderAsset = 0xC5,
    DeltaBlock = 0xC6,
    RawCp437 = 0xFE,
}

impl Opcode {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(Self::Nop),
            0x01 => Some(Self::ClearScreen),
            0x02 => Some(Self::Crlf),
            0x03 => Some(Self::PagePause),
            0x04 => Some(Self::EndOfFrame),
            0xC0 => Some(Self::SetColor),
            0xC1 => Some(Self::RleGlyph),
            0xC2 => Some(Self::RleSpace),
            0xC3 => Some(Self::CursorAbs),
            0xC4 => Some(Self::CursorRel),
            0xC5 => Some(Self::RenderAsset),
            0xC6 => Some(Self::DeltaBlock),
            0xFE => Some(Self::RawCp437),
            _ => None,
        }
    }
}

/// Compiles standard ANSI text/escape sequences into compact MeshANSI bytecode.
pub fn compile_ansi(raw_ansi: &str) -> Result<Vec<u8>, MeshAnsiError> {
    // Stub implementation for compilation logic
    let mut bytecode = Vec::new();
    // For now, write a Nop and clear screen as baseline
    bytecode.push(Opcode::ClearScreen as u8);
    // Add raw character literals
    for byte in raw_ansi.bytes() {
        if byte.is_ascii() && byte >= 0x20 && byte <= 0x7E {
            bytecode.push(byte);
        } else {
            // Escape code handling stub
        }
    }
    bytecode.push(Opcode::EndOfFrame as u8);
    Ok(bytecode)
}

/// Compresses bytecode using Heatshrink LZSS algorithm (W=8, L=4).
pub fn compress_bytecode(bytecode: &[u8]) -> Result<Vec<u8>, MeshAnsiError> {
    // Stub compression logic
    // Heatshrink configuration: window_bits = 8, lookahead_bits = 4
    Ok(bytecode.to_vec()) // Stub: returning uncompressed copy for setup
}

/// Decompresses Heatshrink compressed bytecode.
pub fn decompress_bytecode(compressed: &[u8]) -> Result<Vec<u8>, MeshAnsiError> {
    // Stub decompression logic
    Ok(compressed.to_vec()) // Stub: returning uncompressed copy for setup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opcode_from_u8() {
        assert_eq!(Opcode::from_u8(0x00), Some(Opcode::Nop));
        assert_eq!(Opcode::from_u8(0x01), Some(Opcode::ClearScreen));
        assert_eq!(Opcode::from_u8(0x02), Some(Opcode::Crlf));
        assert_eq!(Opcode::from_u8(0x03), Some(Opcode::PagePause));
        assert_eq!(Opcode::from_u8(0x04), Some(Opcode::EndOfFrame));
        assert_eq!(Opcode::from_u8(0xC0), Some(Opcode::SetColor));
        assert_eq!(Opcode::from_u8(0xC1), Some(Opcode::RleGlyph));
        assert_eq!(Opcode::from_u8(0xC2), Some(Opcode::RleSpace));
        assert_eq!(Opcode::from_u8(0xC3), Some(Opcode::CursorAbs));
        assert_eq!(Opcode::from_u8(0xC4), Some(Opcode::CursorRel));
        assert_eq!(Opcode::from_u8(0xC5), Some(Opcode::RenderAsset));
        assert_eq!(Opcode::from_u8(0xC6), Some(Opcode::DeltaBlock));
        assert_eq!(Opcode::from_u8(0xFE), Some(Opcode::RawCp437));
        assert_eq!(Opcode::from_u8(0xFF), None);
        assert_eq!(Opcode::from_u8(0x55), None);
    }

    #[test]
    fn test_compile_ansi() {
        let compiled = compile_ansi("Hello World\x1b").unwrap();
        assert_eq!(compiled[0], Opcode::ClearScreen as u8);
        assert_eq!(compiled[compiled.len() - 1], Opcode::EndOfFrame as u8);
        
        // Extract inner string content from compiled bytecode
        let content: Vec<u8> = compiled.iter()
            .cloned()
            .filter(|&x| x >= 0x20 && x <= 0x7E)
            .collect();
        assert_eq!(String::from_utf8(content).unwrap(), "Hello World");
    }

    #[test]
    fn test_compression_decompression() {
        let data = vec![0x01, 0x48, 0x45, 0x4c, 0x4c, 0x4f, 0x04];
        let compressed = compress_bytecode(&data).unwrap();
        let decompressed = decompress_bytecode(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_mesh_ansi_error_display() {
        assert_eq!(
            MeshAnsiError::ParseError("invalid sequence".to_string()).to_string(),
            "Failed to parse ANSI sequence: invalid sequence"
        );
        assert_eq!(
            MeshAnsiError::CompressionError("out of memory".to_string()).to_string(),
            "Compression error: out of memory"
        );
        assert_eq!(
            MeshAnsiError::DecompressionError("invalid payload".to_string()).to_string(),
            "Decompression error: invalid payload"
        );
        assert_eq!(
            MeshAnsiError::Io("file not found".to_string()).to_string(),
            "IO error: file not found"
        );
    }
}

