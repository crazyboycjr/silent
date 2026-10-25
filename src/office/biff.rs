//! Spreadsheets saved as a bare BIFF record stream, without the OLE file
//! Excel wraps one in: what old exporters wrote, and Excel still opens.
//! calamine reads only the wrapped kind, so these cells are read here.

use encoding_rs::Encoding;

use super::sheet::{Cell, general};

/// Whether `head` starts with a BIFF beginning-of-file record, of BIFF2
/// to BIFF8.
pub fn is_biff(head: &[u8]) -> bool {
    head.len() >= 4
        && matches!(u16_at(head, 0), Some(0x0009 | 0x0209 | 0x0409 | 0x0809))
        && u16_at(head, 2).is_some_and(|len| (4..=32).contains(&len))
}

/// The cells of a bare BIFF stream with text, by row and column.
pub fn cells(data: &[u8]) -> Option<Vec<((u32, u32), Cell)>> {
    if !is_biff(data) {
        return None;
    }
    let mut encoding: &'static Encoding = encoding_rs::GB18030;
    let mut out = Vec::new();
    let mut at = 0;
    while let (Some(id), Some(size)) = (u16_at(data, at), u16_at(data, at + 2)) {
        let Some(body) = data.get(at + 4..at + 4 + usize::from(size)) else {
            break;
        };
        at += 4 + usize::from(size);
        let place = || Some((u32::from(u16_at(body, 0)?), u32::from(u16_at(body, 2)?)));
        let cell = match id {
            0x000a => break,
            // The code page the 8-bit strings are in.
            0x0042 => {
                if let Some(found) = u16_at(body, 0).and_then(code_page) {
                    encoding = found;
                }
                None
            }
            0x0203 => place().zip(f64_at(body, 6)).map(|(p, n)| (p, number(n))),
            0x0003 => place().zip(f64_at(body, 7)).map(|(p, n)| (p, number(n))),
            0x0002 => place()
                .zip(u16_at(body, 7))
                .map(|(p, n)| (p, number(f64::from(n)))),
            0x027e => place()
                .zip(u32_at(body, 6))
                .map(|(p, v)| (p, number(rk(v)))),
            0x00bd => {
                // A run of RK numbers along one row.
                if let (Some((row, first)), true) = (place(), body.len() >= 6) {
                    for (i, chunk) in body[4..body.len() - 2]
                        .as_chunks::<6>()
                        .0
                        .iter()
                        .enumerate()
                    {
                        if let Some(v) = u32_at(chunk, 2) {
                            out.push(((row, first + i as u32), number(rk(v))));
                        }
                    }
                }
                None
            }
            0x0204 | 0x0004 => {
                let text = if id == 0x0204 {
                    body.get(6..).map(|s| label(s, encoding))
                } else {
                    body.get(7..).and_then(|s| {
                        let len = usize::from(*s.first()?);
                        Some(decode(s.get(1..1 + len)?, encoding))
                    })
                };
                place().zip(text).map(|(p, t)| {
                    (
                        p,
                        Cell {
                            text: t,
                            number: false,
                        },
                    )
                })
            }
            0x0205 | 0x0005 => {
                let offset = if id == 0x0205 { 6 } else { 7 };
                let (value, error) = (body.get(offset), body.get(offset + 1));
                let text = match (value, error) {
                    (Some(v), Some(0)) => Some(if *v == 0 { "FALSE" } else { "TRUE" }.to_owned()),
                    (Some(_), Some(_)) => Some("#ERROR".to_owned()),
                    _ => None,
                };
                place().zip(text).map(|(p, t)| {
                    (
                        p,
                        Cell {
                            text: t,
                            number: false,
                        },
                    )
                })
            }
            // A formula's last value, when it is a number.
            0x0006 | 0x0206 | 0x0406 => place()
                .zip(f64_at(body, 6).filter(|_| u16_at(body, 12) != Some(0xffff)))
                .map(|(p, n)| (p, number(n))),
            _ => None,
        };
        out.extend(cell.filter(|(_, c)| !c.text.is_empty()));
    }
    Some(out)
}

fn number(n: f64) -> Cell {
    Cell {
        text: general(n),
        number: true,
    }
}

/// A BIFF5 string of 8-bit characters, or a BIFF8 one with its flags byte,
/// told apart by their lengths: these files rarely say which BIFF they are.
fn label(s: &[u8], encoding: &'static Encoding) -> String {
    let Some(len) = u16_at(s, 0).map(usize::from) else {
        return String::new();
    };
    let rest = &s[2..];
    match rest.first() {
        _ if rest.len() == len => decode(rest, encoding),
        Some(flags) if rest.len() == len + 1 && flags & 1 == 0 => decode(&rest[1..], encoding),
        Some(flags) if rest.len() > 2 * len && flags & 1 == 1 => {
            let units: Vec<u16> = rest[1..1 + 2 * len]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => decode(
            rest.get(..len.min(rest.len())).unwrap_or_default(),
            encoding,
        ),
    }
}

/// UTF-8 when it is valid, otherwise the file's code page.
fn decode(bytes: &[u8], encoding: &'static Encoding) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => encoding.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

fn code_page(page: u16) -> Option<&'static Encoding> {
    Some(match page {
        936 | 54936 => encoding_rs::GB18030,
        950 => encoding_rs::BIG5,
        932 => encoding_rs::SHIFT_JIS,
        949 => encoding_rs::EUC_KR,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 | 367 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        10000 => encoding_rs::MACINTOSH,
        _ => return None,
    })
}

/// An RK number: an integer or the top of a double, maybe in hundredths.
fn rk(v: u32) -> f64 {
    let n = if v & 2 != 0 {
        f64::from((v as i32) >> 2)
    } else {
        f64::from_bits(u64::from(v & 0xffff_fffc) << 32)
    };
    if v & 1 != 0 { n / 100. } else { n }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn f64_at(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u16, body: &[u8]) -> Vec<u8> {
        let mut out = id.to_le_bytes().to_vec();
        out.extend((body.len() as u16).to_le_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn bare_streams_give_their_cells() {
        let mut data = record(0x0809, &[0, 0, 0x10, 0, 0, 0, 0, 0]);
        // LABEL A1 "名字" in GBK, as BIFF5 writes it.
        let mut label = vec![0, 0, 0, 0, 0x0f, 0, 4, 0];
        label.extend([0xc3, 0xfb, 0xd7, 0xd6]);
        data.extend(record(0x0204, &label));
        // NUMBER B1 2.5.
        let mut number = vec![0, 0, 1, 0, 0x0f, 0];
        number.extend(2.5f64.to_le_bytes());
        data.extend(record(0x0203, &number));
        // RK A2 123 (an integer RK), then MULRK B2:C2 of 1.5 and 7.
        data.extend(record(0x027e, &[1, 0, 0, 0, 0x0f, 0, 0xee, 0x01, 0, 0]));
        let mut mulrk = vec![1, 0, 1, 0];
        mulrk.extend([0x0f, 0]);
        mulrk.extend(((1.5f64.to_bits() >> 32) as u32).to_le_bytes());
        mulrk.extend([0x0f, 0]);
        mulrk.extend((7u32 << 2 | 2).to_le_bytes());
        mulrk.extend([2, 0]);
        data.extend(record(0x00bd, &mulrk));
        data.extend(record(0x000a, &[]));
        let cells = cells(&data).unwrap();
        let text: Vec<_> = cells
            .iter()
            .map(|((r, c), cell)| (*r, *c, cell.text.as_str()))
            .collect();
        assert_eq!(
            text,
            [
                (0, 0, "名字"),
                (0, 1, "2.5"),
                (1, 0, "123"),
                (1, 1, "1.5"),
                (1, 2, "7")
            ]
        );
    }

    #[test]
    fn only_beginning_of_file_records_start_a_stream() {
        assert!(is_biff(&[0x09, 0x08, 0x08, 0x00]));
        assert!(is_biff(&[0x09, 0x00, 0x04, 0x00]));
        assert!(!is_biff(&[0xd0, 0xcf, 0x11, 0xe0]));
        assert!(!is_biff(b"PK\x03\x04"));
    }
}
