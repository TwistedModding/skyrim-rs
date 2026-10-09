//! Address Library reader: formats 1 and 2 (delta stream) and 5 (dense table).

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;

use memmap2::Mmap;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Format(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "address library: {e}"),
            Self::Format(what) => write!(f, "address library: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

enum Table {
    Sorted(Vec<(u32, u32)>),
    Dense { start: usize, count: usize },
}

pub struct VersionDb {
    map: Mmap,
    format: u32,
    version: [u32; 4],
    table: Table,
}

impl VersionDb {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        Self::from_map(map)
    }

    fn from_map(map: Mmap) -> Result<Self, Error> {
        let mut r = Reader { bytes: &map, pos: 0 };
        let format = r.u32()?;
        let version = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        let table = match format {
            1 | 2 => {
                let name_len = r.u32()? as usize;
                r.skip(name_len)?;
                let pointer_size = u64::from(r.u32()?);
                let count = r.u32()? as usize;
                Table::Sorted(decode_entries(&mut r, count, pointer_size)?)
            }
            5 => {
                r.skip(64)?;
                r.skip(8)?;
                let count = r.u32()? as usize;
                let start = r.pos;
                r.skip(count * 4)?;
                Table::Dense { start, count }
            }
            _ => return Err(Error::Format("unknown format")),
        };
        if r.pos != r.bytes.len() {
            return Err(Error::Format("trailing data"));
        }
        Ok(Self { map, format, version, table })
    }

    #[must_use]
    pub fn format(&self) -> u32 {
        self.format
    }

    /// `[major, minor, build, sub]` as written in the file header.
    #[must_use]
    pub fn version(&self) -> [u32; 4] {
        self.version
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match &self.table {
            Table::Sorted(v) => v.len(),
            Table::Dense { count, .. } => *count,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Offset from the module base for an Address Library id.
    #[must_use]
    pub fn offset(&self, id: u64) -> Option<u64> {
        let id = u32::try_from(id).ok()?;
        match &self.table {
            Table::Sorted(v) => v
                .binary_search_by_key(&id, |&(i, _)| i)
                .ok()
                .map(|i| u64::from(v[i].1)),
            Table::Dense { start, count } => {
                let at = *start + (id as usize).checked_mul(4)?;
                if (id as usize) >= *count {
                    return None;
                }
                let bytes: [u8; 4] = self.map[at..at + 4].try_into().ok()?;
                match u32::from_le_bytes(bytes) {
                    0 => None,
                    o => Some(u64::from(o)),
                }
            }
        }
    }

    /// Every assigned `(id, offset)` pair in id order.
    pub fn iter(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        let (sorted, dense) = match &self.table {
            Table::Sorted(v) => (Some(v.iter().map(|&(i, o)| (u64::from(i), u64::from(o)))), None),
            Table::Dense { count, .. } => (None, Some((0..*count as u64).filter_map(|id| Some((id, self.offset(id)?))))),
        };
        sorted.into_iter().flatten().chain(dense.into_iter().flatten())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or(Error::Format("truncated"))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.take(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn delta(&mut self, code: u8, previous: u64) -> Result<u64, Error> {
        Ok(match code {
            0 => self.u64()?,
            1 => previous.wrapping_add(1),
            2 => previous.wrapping_add(u64::from(self.u8()?)),
            3 => previous.wrapping_sub(u64::from(self.u8()?)),
            4 => previous.wrapping_add(u64::from(self.u16()?)),
            5 => previous.wrapping_sub(u64::from(self.u16()?)),
            6 => u64::from(self.u16()?),
            7 => u64::from(self.u32()?),
            _ => return Err(Error::Format("bad entry code")),
        })
    }
}

fn decode_entries(r: &mut Reader<'_>, count: usize, pointer_size: u64) -> Result<Vec<(u32, u32)>, Error> {
    let mut out = Vec::with_capacity(count);
    let (mut id, mut offset) = (0u64, 0u64);
    let mut sorted = true;
    for _ in 0..count {
        let kind = r.u8()?;
        id = r.delta(kind & 0xF, id)?;
        let scaled = kind & 0x80 != 0;
        let base = if scaled { offset / pointer_size } else { offset };
        offset = r.delta((kind >> 4) & 7, base)?;
        if scaled {
            offset *= pointer_size;
        }
        let pair = (
            u32::try_from(id).map_err(|_| Error::Format("id overflow"))?,
            u32::try_from(offset).map_err(|_| Error::Format("offset overflow"))?,
        );
        sorted &= out.last().is_none_or(|&(prev, _)| prev < pair.0);
        out.push(pair);
    }
    if !sorted {
        out.sort_unstable_by_key(|&(i, _)| i);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_codes_decode() {
        let mut bytes = vec![0x00];
        bytes.extend(10u64.to_le_bytes());
        bytes.extend(0x1000u64.to_le_bytes());
        bytes.extend([0x21, 0x10]);
        bytes.extend([0xA1, 0x02]);
        bytes.extend([0x32, 0x05, 0x08]);
        bytes.extend([0x44, 0x00, 0x01, 0x20, 0x00]);
        bytes.extend([0x56, 0x34, 0x12, 0x01, 0x00]);
        bytes.extend([0x67, 0x78, 0x56, 0x34, 0x12, 0x00, 0x02]);
        bytes.extend([0x73, 0x01, 0x40, 0x30, 0x20, 0x10]);
        let mut r = Reader { bytes: &bytes, pos: 0 };
        let got = decode_entries(&mut r, 8, 8).unwrap();
        assert_eq!(r.pos, bytes.len());
        assert_eq!(
            got,
            [
                (10, 0x1000),
                (11, 0x1010),
                (12, 0x1020),
                (17, 0x1018),
                (0x111, 0x1038),
                (0x1234, 0x1037),
                (0x1234_5677, 0x1020_3040),
                (0x1234_5678, 0x200),
            ]
        );
    }

    #[test]
    fn unsorted_input_is_sorted() {
        let mut bytes = vec![0x00];
        bytes.extend(5u64.to_le_bytes());
        bytes.extend(1u64.to_le_bytes());
        bytes.extend([0x13, 0x02]);
        let mut r = Reader { bytes: &bytes, pos: 0 };
        assert_eq!(decode_entries(&mut r, 2, 8).unwrap(), [(3, 2), (5, 1)]);
    }
}
