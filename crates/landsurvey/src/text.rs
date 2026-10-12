//! Text decoding for survey input files — pure, `std` only.
//!
//! Point files arrive from data collectors and office software in whatever
//! encoding the exporter used: UTF-8 (with or without a BOM), UTF-16 from
//! Windows tools that "save as Unicode", or Windows-1252 from older Windows
//! software, where a `°` or `½` in a description is enough to make the file
//! invalid UTF-8. [`decode`] reads all of these rather than refusing the file.

/// Windows-1252 code points for bytes `0x80..=0x9F`. The five bytes the code
/// page leaves undefined map to the C1 control of the same value, as the
/// WHATWG encoding standard does.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Decode a text file's bytes. A UTF-8 or UTF-16 byte-order mark selects that
/// encoding (and is dropped); otherwise the bytes are read as UTF-8 when valid
/// and as Windows-1252 when not. Never fails: Windows-1252 maps every byte.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, u16::from_be_bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| unit([c[0], c[1]]));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn cp1252(b: u8) -> char {
    match b {
        0x80..=0x9F => CP1252_HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passes_through_and_loses_its_bom() {
        assert_eq!(decode("1,5000,1000,100,Ø FENCE".as_bytes()), "1,5000,1000,100,Ø FENCE");
        assert_eq!(decode(b"\xEF\xBB\xBF1,2"), "1,2");
    }

    #[test]
    fn invalid_utf8_is_read_as_windows_1252() {
        // "IP 45° ½in – “set”" as Windows-1252.
        let bytes = b"IP 45\xB0 \xBDin \x96 \x93set\x94";
        assert_eq!(decode(bytes), "IP 45° ½in – “set”");
    }

    #[test]
    fn utf16_with_either_bom() {
        let text = "1,5000,1000,100,Ø\r\n";
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(decode(&le), text);
        assert_eq!(decode(&be), text);
    }
}
