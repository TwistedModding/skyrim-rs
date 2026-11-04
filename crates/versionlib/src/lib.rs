//! Address Library reader: formats 1 and 2 (delta stream) and 5 (dense table), the running game's
//! database, opened once per DLL, and the runtime [`profile`] that names it and translates ids.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use memmap2::Mmap;
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW};

pub mod manifest;
pub mod profile;

use profile::{Profile, Row};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Format(&'static str),
    /// The running runtime's profile could not be read; the text names the runtime and the file.
    Profile(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "address library: {e}"),
            Self::Format(what) => write!(f, "address library: {what}"),
            Self::Profile(what) => f.write_str(what),
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

/// Base address of the executable that started this process.
#[must_use]
pub fn module_base() -> usize {
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}

/// The file of the module whose image holds `address`.
pub fn module_file(address: *const core::ffi::c_void) -> io::Result<PathBuf> {
    let mut module = core::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, address.cast(), &raw mut module) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut path = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), 1024) } as usize;
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&path[..len])))
}

/// `<exe dir>\Data\SKSE\Plugins`.
pub fn plugins_dir() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name("Data").join("SKSE").join("Plugins"))
}

/// The running game's Address Library with the executable's base address. Ids here are AE ids:
/// they go through the runtime's profile before they reach the file.
pub struct Running {
    db: VersionDb,
    base: usize,
    path: PathBuf,
    profile: &'static Profile,
}

impl Running {
    #[must_use]
    pub fn base(&self) -> usize {
        self.base
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn profile(&self) -> &'static Profile {
        self.profile
    }

    #[must_use]
    pub fn db(&self) -> &VersionDb {
        &self.db
    }

    /// Why a DLL with these bytes must not run on this game, with all it lacks; `None` when it may.
    #[must_use]
    pub fn refusal(&self, pe: &[u8]) -> Option<String> {
        manifest::refusal(&manifest::fit(pe, self.profile, Some(&self.db)), &self.profile.name())
    }

    /// The module-relative offset of an AE id, `None` when the profile has no usable row for it or
    /// the file does not hold it.
    #[must_use]
    pub fn offset(&self, ae_id: u64) -> Option<u64> {
        self.db.offset(match self.profile.id(u32::try_from(ae_id).ok()?) {
            Row::Same => ae_id,
            Row::Here(id) => u64::from(id),
            Row::Missing | Row::Unverified => return None,
        })
    }

    /// The live address of an AE id, `None` as for [`Self::offset`].
    #[must_use]
    pub fn address(&self, ae_id: u64) -> Option<usize> {
        Some(self.base + usize::try_from(self.offset(ae_id)?).ok()?)
    }

    /// The live address of the site `offset` bytes into the function at an AE id on 1.7.104,
    /// wherever the profile places it; `None` when the profile has no place for it or no usable id.
    #[must_use]
    pub fn site(&self, ae_id: u64, offset: u32) -> Option<usize> {
        let here = match self.profile.site(u32::try_from(ae_id).ok()?, offset) {
            Row::Same => offset,
            Row::Here(site) => site.offset,
            Row::Missing | Row::Unverified => return None,
        };
        Some(self.address(ae_id)? + usize::try_from(here).ok()?)
    }
}

static RUNNING: OnceLock<Running> = OnceLock::new();

/// The running game's database, the file its profile names, opened by the first call in this DLL
/// and shared by every later one.
pub fn running() -> Result<&'static Running, Error> {
    if let Some(running) = RUNNING.get() {
        return Ok(running);
    }
    let profile = profile::current().map_err(Error::Profile)?;
    let path = plugins_dir()?.join(profile.address_library());
    let db = VersionDb::open(&path).map_err(|e| match e {
        Error::Io(io) => Error::Io(io::Error::new(io.kind(), format!("{}: {io}", path.display()))),
        other => other,
    })?;
    Ok(RUNNING.get_or_init(|| Running { db, base: module_base(), path, profile }))
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
