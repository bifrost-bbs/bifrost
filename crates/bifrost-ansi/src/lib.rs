//! MeshANSI compiler and compression library.
//! Handles converting ANSI sequences into 1-byte opcodes, differential drawing,
//! and Heatshrink LZSS compression.

use bifrost_compression::{
    compress_adaptive, decompress_adaptive, CompressionDictionary, CompressionError, Heatshrink,
};
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

impl From<CompressionError> for MeshAnsiError {
    fn from(err: CompressionError) -> Self {
        match err {
            CompressionError::CompressionOutputFull | CompressionError::InvalidParameters(_) => {
                MeshAnsiError::CompressionError(err.to_string())
            }
            CompressionError::DecompressionOutputFull
            | CompressionError::CorruptDictionary(_)
            | CompressionError::InvalidTokenReference(_)
            | CompressionError::DictionaryCrcMismatch { .. } => {
                MeshAnsiError::DecompressionError(err.to_string())
            }
        }
    }
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
    RenderTemplate = 0xC7,
    RenderMenu = 0xC8,
    FormStart = 0xD0,
    FormField = 0xD1,
    FormSubmit = 0xD2,
    FormEnd = 0xD3,
    DictToken = 0xFD,
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
            0xC7 => Some(Self::RenderTemplate),
            0xC8 => Some(Self::RenderMenu),
            0xD0 => Some(Self::FormStart),
            0xD1 => Some(Self::FormField),
            0xD2 => Some(Self::FormSubmit),
            0xD3 => Some(Self::FormEnd),
            0xFD => Some(Self::DictToken),
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
        if byte.is_ascii() && (0x20..=0x7E).contains(&byte) {
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
    let hs = Heatshrink::new(8, 4)?;
    Ok(hs.compress(bytecode)?)
}

/// Decompresses Heatshrink compressed bytecode.
pub fn decompress_bytecode(compressed: &[u8]) -> Result<Vec<u8>, MeshAnsiError> {
    let hs = Heatshrink::new(8, 4)?;
    Ok(hs.decompress(compressed)?)
}

/// Compresses bytecode adaptively with optional dictionary and fallback guard.
/// Returns (flags, payload) where flags:
/// - 0x00: Raw fallback (guaranteed never to expand)
/// - 0x02: Heatshrink LZSS only
/// - 0x04: Dictionary only
/// - 0x06: Dictionary + Heatshrink LZSS
pub fn compress_bytecode_adaptive(
    bytecode: &[u8],
    dict: Option<&CompressionDictionary>,
) -> (u8, Vec<u8>) {
    compress_adaptive(bytecode, dict, 8, 4)
}

/// Decompresses bytecode adaptively based on message flags.
/// Substitutes positional placeholders `{0}`, `{1}`, ... in a template string.
pub fn substitute_template(template: &str, params: &[String]) -> String {
    let mut result = template.to_string();
    for (idx, val) in params.iter().enumerate() {
        let placeholder = format!("{{{}}}", idx);
        result = result.replace(&placeholder, val);
    }
    result
}

/// Representation of an interactive button in a menu asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuButtonDef {
    pub tag: String,
    pub id: String,
    pub label: String,
    pub col: u8,
    pub row: u8,
    pub key: Option<char>,
}

/// Representation of a parsed menu asset definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuAssetDef {
    pub form_id: u8,
    pub field_fg: Option<u8>,
    pub field_bg: Option<u8>,
    pub submit_fg: Option<u8>,
    pub submit_bg: Option<u8>,
    pub align: Option<String>,
    pub buttons: Vec<MenuButtonDef>,
}

/// Parses a menu asset definition from CSV format.
pub fn parse_menu_csv(content: &str) -> MenuAssetDef {
    let mut form_id = 1;
    let mut field_fg = None;
    let mut field_bg = None;
    let mut submit_fg = None;
    let mut submit_bg = None;
    let mut align = None;
    let mut buttons = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            let comment_body = trimmed.trim_start_matches('#').trim();
            if let Some((k, v)) = comment_body.split_once('=') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim();
                match k.as_str() {
                    "form_id" => {
                        if let Ok(val) = v.parse::<u8>() {
                            form_id = val;
                        }
                    }
                    "field_fg" => {
                        if let Ok(val) = v.parse::<u8>() {
                            field_fg = Some(val);
                        }
                    }
                    "field_bg" => {
                        if let Ok(val) = v.parse::<u8>() {
                            field_bg = Some(val);
                        }
                    }
                    "submit_fg" => {
                        if let Ok(val) = v.parse::<u8>() {
                            submit_fg = Some(val);
                        }
                    }
                    "submit_bg" => {
                        if let Ok(val) = v.parse::<u8>() {
                            submit_bg = Some(val);
                        }
                    }
                    "align" => {
                        align = Some(v.to_ascii_lowercase());
                    }
                    _ => {}
                }
            }
            continue;
        }

        let parts: Vec<&str> = trimmed.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 5 {
            let tag = parts[0].to_string();
            let id = parts[1].to_string();
            let label = parts[2].to_string();
            let col = parts[3].parse::<u8>().unwrap_or(0);
            let row = parts[4].parse::<u8>().unwrap_or(0);
            let key = if parts.len() >= 6 && !parts[5].is_empty() {
                parts[5].chars().next()
            } else {
                None
            };
            buttons.push(MenuButtonDef {
                tag,
                id,
                label,
                col,
                row,
                key,
            });
        }
    }

    MenuAssetDef {
        form_id,
        field_fg,
        field_bg,
        submit_fg,
        submit_bg,
        align,
        buttons,
    }
}

/// Decompresses bytecode adaptively based on message flags.
pub fn decompress_bytecode_adaptive(
    flags: u8,
    payload: &[u8],
    dict: Option<&CompressionDictionary>,
) -> Result<Vec<u8>, MeshAnsiError> {
    Ok(decompress_adaptive(flags, payload, dict, 8, 4)?)
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
        assert_eq!(Opcode::from_u8(0xC7), Some(Opcode::RenderTemplate));
        assert_eq!(Opcode::from_u8(0xC8), Some(Opcode::RenderMenu));
        assert_eq!(Opcode::from_u8(0xFE), Some(Opcode::RawCp437));
        assert_eq!(Opcode::from_u8(0xFF), None);
        assert_eq!(Opcode::from_u8(0x55), None);
    }

    #[test]
    fn test_substitute_template() {
        let tmpl = "Sector: {0} [{1},{2}] Turns: {3}";
        let params = vec![
            "10".to_string(),
            "04".to_string(),
            "08".to_string(),
            "100".to_string(),
        ];
        let res = substitute_template(tmpl, &params);
        assert_eq!(res, "Sector: 10 [04,08] Turns: 100");
    }

    #[test]
    fn test_parse_menu_csv() {
        let csv_data = "# form_id=15\n# field_fg=14\n# submit_bg=1\n# align=bottom_center\nnorth,wn,North,2,14,N\nsouth,ws,South,10,14,S\n";
        let def = parse_menu_csv(csv_data);
        assert_eq!(def.form_id, 15);
        assert_eq!(def.field_fg, Some(14));
        assert_eq!(def.submit_bg, Some(1));
        assert_eq!(def.align.as_deref(), Some("bottom_center"));
        assert_eq!(def.buttons.len(), 2);
        assert_eq!(def.buttons[0].tag, "north");
        assert_eq!(def.buttons[0].id, "wn");
        assert_eq!(def.buttons[0].label, "North");
        assert_eq!(def.buttons[0].col, 2);
        assert_eq!(def.buttons[0].row, 14);
        assert_eq!(def.buttons[0].key, Some('N'));
        assert_eq!(def.buttons[1].tag, "south");
        assert_eq!(def.buttons[1].key, Some('S'));
    }

    #[test]
    fn test_compile_ansi() {
        let compiled = compile_ansi("Hello World\x1b").unwrap();
        assert_eq!(compiled[0], Opcode::ClearScreen as u8);
        assert_eq!(compiled[compiled.len() - 1], Opcode::EndOfFrame as u8);

        // Extract inner string content from compiled bytecode
        let content: Vec<u8> = compiled
            .iter()
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
