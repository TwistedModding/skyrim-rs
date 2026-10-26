//! One file out of a Bethesda archive (BSA version 105, Skyrim Special Edition): the header, the
//! folder records, each folder's name and file records, then the file names, all read from the
//! front of the archive without loading the rest.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

const VERSION: u32 = 105;
const HEADER: usize = 36;
const FOLDER_RECORD: usize = 24;
const FILE_RECORD: usize = 16;
const HAS_FOLDER_NAMES: u32 = 0x1;
const HAS_FILE_NAMES: u32 = 0x2;
const COMPRESSED: u32 = 0x4;
const EMBEDDED_NAMES: u32 = 0x100;
/// In a file record's size: the file's compression is the opposite of the archive's default.
const TOGGLE_COMPRESSION: u32 = 0x4000_0000;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The bytes of `folder\name` (case-insensitive, backslashes) in the archive at `path`. Compressed
/// entries are not read: the vanilla interface archive stores its files uncompressed.
pub(crate) fn read(path: &Path, folder: &str, name: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut header = [0u8; HEADER];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    let field = |i: usize| u32_at(&header, 4 * i).unwrap_or(0) as usize;
    if &header[..4] != b"BSA\0" || field(1) != VERSION as usize {
        return Err(format!("{}: not a version {VERSION} archive", path.display()));
    }
    let (flags, folders, files) = (u32::try_from(field(3)).unwrap_or(0), field(4), field(5));
    if flags & (HAS_FOLDER_NAMES | HAS_FILE_NAMES) != HAS_FOLDER_NAMES | HAS_FILE_NAMES {
        return Err("the archive has no names".into());
    }
    let directory = HEADER + folders * FOLDER_RECORD + field(6) + folders + files * FILE_RECORD + field(7);
    let mut dir = vec![0u8; directory - HEADER];
    file.read_exact(&mut dir).map_err(|e| e.to_string())?;
    let bad = || "truncated directory".to_owned();
    let mut at = folders * FOLDER_RECORD;
    let mut records = Vec::with_capacity(files);
    for f in 0..folders {
        let count = u32_at(&dir, f * FOLDER_RECORD + 8).ok_or_else(bad)? as usize;
        let len = usize::from(*dir.get(at).ok_or_else(bad)?);
        let folder_name = dir.get(at + 1..at + len).ok_or_else(bad)?;
        at += 1 + len;
        for _ in 0..count {
            let size = u32_at(&dir, at + 8).ok_or_else(bad)?;
            let offset = u32_at(&dir, at + 12).ok_or_else(bad)?;
            records.push((folder_name, size, offset));
            at += FILE_RECORD;
        }
    }
    let wanted = folder.as_bytes();
    for (folder_name, size, offset) in records {
        let end = dir[at..].iter().position(|&b| b == 0).ok_or_else(bad)?;
        let file_name = &dir[at..at + end];
        at += end + 1;
        if !folder_name.eq_ignore_ascii_case(wanted) || !file_name.eq_ignore_ascii_case(name.as_bytes()) {
            continue;
        }
        if (flags & COMPRESSED != 0) != (size & TOGGLE_COMPRESSION != 0) {
            return Err(format!("{folder}\\{name} is compressed"));
        }
        let mut data = vec![0u8; (size & !TOGGLE_COMPRESSION & !0x8000_0000) as usize];
        file.seek(SeekFrom::Start(u64::from(offset))).map_err(|e| e.to_string())?;
        file.read_exact(&mut data).map_err(|e| e.to_string())?;
        if flags & EMBEDDED_NAMES != 0 {
            let skip = 1 + usize::from(*data.first().ok_or_else(bad)?);
            data.drain(..skip.min(data.len()));
        }
        return Ok(data);
    }
    Err(format!("no {folder}\\{name} in {}", path.display()))
}
