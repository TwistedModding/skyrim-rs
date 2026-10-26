//! Formatting without the heap: a writer over a fixed buffer, and one that flushes to a file handle.

use core::fmt;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::WriteFile;

/// A `fmt::Write` over a byte buffer; what does not fit is dropped and `truncated` set.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
    truncated: bool,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0, truncated: false }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        crate::record::text(self.buf, self.len)
    }
}

impl fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = self.buf.len() - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        if n < s.len() {
            self.truncated = true;
        }
        Ok(())
    }
}

/// A `fmt::Write` that drops everything: the probes run through it once before the summary, so the
/// rules see what they would see in the sections below.
pub struct Discard;

impl fmt::Write for Discard {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Ok(())
    }
}

/// A `fmt::Write` that writes through a buffer to an open file; `flush` after each section so a
/// report that stops midway still leaves what was written.
pub struct FileWriter<'a> {
    handle: HANDLE,
    buf: &'a mut [u8],
    len: usize,
    pub failed: bool,
}

impl<'a> FileWriter<'a> {
    pub fn new(handle: HANDLE, buf: &'a mut [u8]) -> Self {
        Self { handle, buf, len: 0, failed: false }
    }

    pub fn flush(&mut self) {
        let mut done = 0;
        while done < self.len {
            let mut written = 0u32;
            let ok = unsafe { WriteFile(self.handle, self.buf[done..self.len].as_ptr(), u32::try_from(self.len - done).unwrap_or(u32::MAX), &raw mut written, core::ptr::null_mut()) };
            if ok == 0 || written == 0 {
                self.failed = true;
                break;
            }
            done += written as usize;
        }
        self.len = 0;
    }
}

impl fmt::Write for FileWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut rest = s.as_bytes();
        while !rest.is_empty() {
            if self.len == self.buf.len() {
                self.flush();
            }
            let n = rest.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&rest[..n]);
            self.len += n;
            rest = &rest[n..];
        }
        Ok(())
    }
}

/// Writes `len` UTF-16 units as UTF-8 into `out`, replacing what is not valid; returns the bytes stored.
pub(crate) fn utf16_into(units: &[u16], out: &mut [u8]) -> usize {
    let mut len = 0;
    for c in char::decode_utf16(units.iter().copied()).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)) {
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp);
        if len + s.len() > out.len() {
            break;
        }
        out[len..len + s.len()].copy_from_slice(s.as_bytes());
        len += s.len();
    }
    len
}
