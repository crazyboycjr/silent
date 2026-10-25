//! Compound files (OLE2), the container of the binary formats of Office 97
//! to 2003, read the way Office reads them: forgiving the slips some writers
//! make, such as end-of-chain marks where free ones belong, or junk in the
//! unused end of the FAT, which stricter readers refuse. The file is mapped,
//! and only the sectors of the streams asked for are read.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use super::{u16_at, u32_at};

/// Sector numbers from here up are marks, not sectors.
const MARKS: u32 = 0xFFFF_FFFA;
/// Streams are read up to this size.
const MOST: usize = 256 << 20;

pub struct Compound<D> {
    data: D,
    shift: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    /// The mini stream, where streams under the cutoff live.
    mini: Vec<u8>,
    cutoff: u64,
    /// Every stream: its name, first sector and size, and whether it sits
    /// at the top of the file rather than in a storage below.
    streams: Vec<(String, u32, u64, bool)>,
}

impl Compound<Mmap> {
    pub fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        // Safety: the map is only read. A file cut short while it is read
        // would fault, as it would for any program that maps files.
        let map = unsafe { Mmap::map(&file) }.ok()?;
        Self::parse(map)
    }
}

impl<D: AsRef<[u8]>> Compound<D> {
    pub fn parse(data: D) -> Option<Self> {
        let bytes = data.as_ref();
        if !super::is_ole(bytes) {
            return None;
        }
        let shift = u32::from(u16_at(bytes, 0x1E)?);
        if !(9..=12).contains(&shift) {
            return None;
        }
        let mut compound = Compound {
            shift,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini: Vec::new(),
            cutoff: u64::from(u32_at(bytes, 0x38)?),
            streams: Vec::new(),
            data,
        };
        compound.read_fat()?;
        let bytes = compound.data.as_ref();
        let directory = compound.chain(u32_at(bytes, 0x30)?, usize::MAX);
        let entries: Vec<&[u8]> = directory
            .as_chunks::<128>()
            .0
            .iter()
            .map(|e| e.as_slice())
            .collect();
        let root = entries.first()?;
        compound.mini_fat = compound
            .chain(u32_at(bytes, 0x3C)?, usize::MAX)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        let mini_size = u64_at(root, 120)? as usize;
        compound.mini = compound.chain(u32_at(root, 116)?, mini_size);
        // The root's children are a tree of siblings, which tells the top
        // streams from those of an embedded document below.
        let mut top = vec![false; entries.len()];
        let mut pending = vec![u32_at(root, 76)?];
        while let Some(id) = pending.pop() {
            match (entries.get(id as usize), top.get_mut(id as usize)) {
                (Some(entry), Some(seen)) if !*seen => {
                    *seen = true;
                    pending.extend([u32_at(entry, 68)?, u32_at(entry, 72)?]);
                }
                _ => {}
            }
        }
        for (entry, top) in entries.iter().zip(top) {
            if entry[66] == 2 {
                let len = usize::from(u16_at(entry, 64)?).clamp(2, 64) / 2 - 1;
                let units = (0..len).filter_map(|i| u16_at(entry, 2 * i));
                let name = char::decode_utf16(units).flatten().collect();
                let (start, size) = (u32_at(entry, 116)?, u64_at(entry, 120)?);
                compound.streams.push((name, start, size, top));
            }
        }
        Some(compound)
    }

    fn sector_size(&self) -> usize {
        1 << self.shift
    }

    /// Sector `n`, after the header's sector.
    fn sector(&self, n: u32) -> Option<&[u8]> {
        let size = self.sector_size();
        let at = (n as usize).checked_add(1)?.checked_mul(size)?;
        self.data.as_ref().get(at..at + size)
    }

    /// The FAT, from the sectors the DIFAT lists: 109 in the header, then a
    /// chain of DIFAT sectors.
    fn read_fat(&mut self) -> Option<()> {
        let bytes = self.data.as_ref();
        let mut sectors: Vec<u32> = (0..109)
            .filter_map(|i| u32_at(bytes, 0x4C + 4 * i))
            .filter(|&s| s < MARKS)
            .collect();
        let per = self.sector_size() / 4;
        let mut next = u32_at(bytes, 0x44)?;
        let mut hops = 0;
        while next < MARKS && hops < 1 << 16 {
            let Some(difat) = self.sector(next) else {
                break;
            };
            sectors.extend(
                (0..per - 1)
                    .filter_map(|i| u32_at(difat, 4 * i))
                    .filter(|&s| s < MARKS),
            );
            next = u32_at(difat, 4 * (per - 1))?;
            hops += 1;
        }
        let fat: Vec<u32> = sectors
            .iter()
            .filter_map(|&s| self.sector(s))
            .flat_map(|s| s.as_chunks::<4>().0)
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        self.fat = fat;
        Some(())
    }

    /// The bytes of the chain of sectors from `start`, up to `len`. A chain
    /// that loops or points outside the FAT ends there.
    fn chain(&self, start: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut next = start;
        let mut hops = 0;
        while next < MARKS && out.len() < len.min(MOST) && hops <= self.fat.len() {
            let Some(sector) = self.sector(next) else {
                break;
            };
            out.extend_from_slice(sector);
            next = self.fat.get(next as usize).copied().unwrap_or(MARKS);
            hops += 1;
        }
        out.truncate(len);
        out
    }

    /// The same in the mini stream, 64 bytes a sector.
    fn mini_chain(&self, start: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut next = start;
        let mut hops = 0;
        while next < MARKS && out.len() < len && hops <= self.mini_fat.len() {
            let at = next as usize * 64;
            let Some(sector) = self.mini.get(at..at + 64) else {
                break;
            };
            out.extend_from_slice(sector);
            next = self.mini_fat.get(next as usize).copied().unwrap_or(MARKS);
            hops += 1;
        }
        out.truncate(len);
        out
    }

    /// The stream called `name` at the top of the file, or, when the
    /// directory's tree is broken, anywhere.
    pub fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let named = |top: bool| {
            self.streams
                .iter()
                .find(|(n, .., t)| n == name && (*t || !top))
        };
        let &(_, start, size, _) = named(true).or_else(|| named(false))?;
        let len = usize::try_from(size).ok()?.min(MOST);
        Some(if size < self.cutoff {
            self.mini_chain(start, len)
        } else {
            self.chain(start, len)
        })
    }
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u32 = 0xFFFF_FFFE;
    const FREE: u32 = 0xFFFF_FFFF;

    /// A compound file of `streams`, as Office writes one; with `sloppy`, with
    /// the slips some writers make: end-of-chain marks in the DIFAT's and the
    /// directory's empty places, and junk at the FAT's unused end.
    fn compound(streams: &[(&str, Vec<u8>)], sloppy: bool) -> Vec<u8> {
        // Short streams go in the mini stream, long ones in sectors.
        let mut mini = Vec::new();
        let mut mini_fat = Vec::new();
        let mut placed = Vec::new();
        let mut sectors: Vec<Vec<u8>> = Vec::new();
        let mut fat: Vec<u32> = Vec::new();
        let put = |data: &[u8], fat: &mut Vec<u32>, sectors: &mut Vec<Vec<u8>>| {
            let first = sectors.len() as u32;
            for (i, chunk) in data.chunks(512).enumerate() {
                let mut sector = chunk.to_vec();
                sector.resize(512, 0);
                sectors.push(sector);
                fat.push(if (i + 1) * 512 < data.len() {
                    first + i as u32 + 1
                } else {
                    END
                });
            }
            first
        };
        for (_, data) in streams {
            if data.len() < 4096 {
                let first = (mini.len() / 64) as u32;
                for (i, chunk) in data.chunks(64).enumerate() {
                    mini.extend(chunk);
                    mini.resize(mini.len().next_multiple_of(64), 0);
                    mini_fat.push(if (i + 1) * 64 < data.len() {
                        first + i as u32 + 1
                    } else {
                        END
                    });
                }
                placed.push(first);
            } else {
                placed.push(put(data, &mut fat, &mut sectors));
            }
        }
        let mini_start = if mini.is_empty() {
            END
        } else {
            put(&mini, &mut fat, &mut sectors)
        };
        let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mini_fat_start = put(&mini_fat_bytes, &mut fat, &mut sectors);
        // The directory: the root, then the streams as a chain of right siblings.
        let entry = |name: &str, kind: u8, child: u32, right: u32, start: u32, size: u64| {
            let mut e = vec![0u8; 128];
            let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            e[..units.len()].copy_from_slice(&units);
            e[64..66].copy_from_slice(&((units.len() + 2) as u16).to_le_bytes());
            e[66] = kind;
            e[67] = 1;
            let none = if sloppy { END } else { FREE };
            e[68..72].copy_from_slice(&none.to_le_bytes());
            e[72..76].copy_from_slice(&right.to_le_bytes());
            e[76..80].copy_from_slice(&child.to_le_bytes());
            e[116..120].copy_from_slice(&start.to_le_bytes());
            e[120..128].copy_from_slice(&size.to_le_bytes());
            e
        };
        let none = if sloppy { END } else { FREE };
        let mut directory = entry("Root Entry", 5, 1, none, mini_start, mini.len() as u64);
        for (i, (name, data)) in streams.iter().enumerate() {
            let right = if i + 1 < streams.len() {
                i as u32 + 2
            } else {
                none
            };
            directory.extend(entry(name, 2, none, right, placed[i], data.len() as u64));
        }
        let directory_start = put(&directory, &mut fat, &mut sectors);
        let fat_sector = sectors.len() as u32;
        fat.push(0xFFFF_FFFD);
        let mut fat_bytes: Vec<u8> = fat.iter().flat_map(|v| v.to_le_bytes()).collect();
        fat_bytes.resize(512, if sloppy { 0x77 } else { 0xFF });
        sectors.push(fat_bytes);
        let mut header = vec![0u8; 512];
        header[..8].copy_from_slice(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
        header[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
        header[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
        header[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
        header[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
        header[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
        header[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
        header[0x30..0x34].copy_from_slice(&directory_start.to_le_bytes());
        header[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
        header[0x3C..0x40].copy_from_slice(&mini_fat_start.to_le_bytes());
        header[0x44..0x48].copy_from_slice(&END.to_le_bytes());
        for i in 0..109 {
            let value = if i == 0 { fat_sector } else { none };
            header[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&value.to_le_bytes());
        }
        let mut out = header;
        for sector in sectors {
            out.extend(sector);
        }
        out
    }

    #[test]
    fn streams_read_from_sectors_and_the_mini_stream() {
        let long: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect();
        let short = b"Current user record".to_vec();
        for sloppy in [false, true] {
            let file = compound(
                &[
                    ("WordDocument", long.clone()),
                    ("Current User", short.clone()),
                ],
                sloppy,
            );
            let compound = Compound::parse(file).unwrap();
            assert_eq!(compound.stream("WordDocument").unwrap(), long);
            assert_eq!(compound.stream("Current User").unwrap(), short);
            assert_eq!(compound.stream("1Table"), None);
        }
    }

    #[test]
    fn a_stream_cut_off_from_the_tree_is_still_found() {
        let table = vec![7u8; 5000];
        let mut file = compound(
            &[("WordDocument", vec![1; 5000]), ("1Table", table.clone())],
            false,
        );
        // The first stream's right sibling, the only way to the second, is lost.
        let name: Vec<u8> = "WordDocument"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let at = file.windows(name.len()).position(|w| w == name).unwrap();
        file[at + 72..at + 76].copy_from_slice(&END.to_le_bytes());
        let compound = Compound::parse(file).unwrap();
        assert_eq!(compound.stream("1Table").unwrap(), table);
    }

    #[test]
    fn other_files_are_not_compound() {
        assert!(Compound::parse(b"PK\x03\x04 a zip".to_vec()).is_none());
        let mut truncated = compound(&[("WordDocument", vec![1; 5000])], false);
        truncated.truncate(1024);
        // The header reads; the stream's sectors are gone.
        let compound = Compound::parse(truncated);
        assert!(compound.is_none_or(|c| c.stream("WordDocument").is_none_or(|s| s.len() < 5000)));
    }
}
