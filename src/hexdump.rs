//! Lines as `hexdump -C` prints them: the offset, sixteen bytes in two
//! groups of eight, and the printable ones between bars.

use std::fmt::Write;

pub const BYTES_PER_LINE: usize = 16;

/// How many lines a dump of `len` bytes has, the closing offset included.
pub fn line_count(len: u64) -> u64 {
    len.div_ceil(BYTES_PER_LINE as u64) + 1
}

/// Hex digits in the offset column: eight, more for files past 4 GiB, so
/// that the columns stay aligned all the way down.
pub fn offset_digits(len: u64) -> usize {
    let bits = 64 - len.leading_zeros() as usize;
    bits.div_ceil(4).max(8)
}

/// Line `line` of the dump of `bytes`; the last line is the closing offset.
pub fn line(bytes: &[u8], line: u64, digits: usize) -> String {
    let offset = (line * BYTES_PER_LINE as u64).min(bytes.len() as u64);
    let mut out = String::with_capacity(digits + 70);
    let _ = write!(out, "{offset:0digits$x}");
    let Some(start) = usize::try_from(offset).ok().filter(|&s| s < bytes.len()) else {
        return out;
    };
    let chunk = &bytes[start..bytes.len().min(start + BYTES_PER_LINE)];
    out.push(' ');
    for i in 0..BYTES_PER_LINE {
        if i % 8 == 0 {
            out.push(' ');
        }
        match chunk.get(i) {
            Some(byte) => {
                let _ = write!(out, "{byte:02x} ");
            }
            None => out.push_str("   "),
        }
    }
    out.push_str(" |");
    out.extend(chunk.iter().map(|&b| printable(b)));
    out.push('|');
    out
}

pub fn printable(byte: u8) -> char {
    if (0x20..0x7f).contains(&byte) {
        byte as char
    } else {
        '.'
    }
}

/// Where the parts of a line start: the hex bytes, and the bars around the
/// characters.
pub fn columns(digits: usize) -> (usize, usize) {
    let hex = digits + 2;
    (hex, hex + 3 * BYTES_PER_LINE + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_hexdump() {
        let bytes = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0>\0hello".to_vec();
        let digits = offset_digits(bytes.len() as u64);
        assert_eq!(line_count(bytes.len() as u64), 3);
        assert_eq!(
            line(&bytes, 0, digits),
            "00000000  7f 45 4c 46 02 01 01 00  00 00 00 00 00 00 00 00  |.ELF............|"
        );
        assert_eq!(
            line(&bytes, 1, digits),
            "00000010  03 00 3e 00 68 65 6c 6c  6f                       |..>.hello|"
        );
        assert_eq!(line(&bytes, 2, digits), "00000019");
    }

    #[test]
    fn whole_lines_end_with_the_length() {
        let bytes = [0x41u8; 32];
        assert_eq!(line_count(32), 3);
        assert_eq!(line(&bytes, 2, 8), "00000020");
        assert_eq!(line_count(0), 1);
    }

    #[test]
    fn offsets_widen_past_four_gibibytes() {
        assert_eq!(offset_digits(0), 8);
        assert_eq!(offset_digits(0xffff_ffff), 8);
        assert_eq!(offset_digits(0x1_0000_0000), 9);
        assert!(line(&[7; 20], 0, 9).starts_with("000000000  07 07 "));
    }

    #[test]
    fn columns_point_at_the_parts() {
        let text = line(b"abc", 0, 8);
        let (hex, bar) = columns(8);
        assert_eq!(&text[hex..hex + 2], "61");
        assert_eq!(&text[bar..bar + 4], "|abc");
    }
}
