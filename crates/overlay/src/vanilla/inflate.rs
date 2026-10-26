//! A zlib stream (RFC 1950) holding DEFLATE data (RFC 1951), the compression of a `CWS` SWF file.

const LENGTH_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// The order the code length code lengths are stored in.
const CLEN_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> Option<u32> {
        while self.count < n {
            self.buf |= u32::from(*self.data.get(self.pos)?) << self.count;
            self.pos += 1;
            self.count += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.count -= n;
        Some(v)
    }

    fn align(&mut self) {
        self.buf = 0;
        self.count = 0;
    }
}

/// A canonical Huffman code: how many codes of each length, and the symbols in code order.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Self {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for len in 1..16 {
            offsets[len] = offsets[len - 1] + counts[len - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offsets[usize::from(l)])] = u16::try_from(symbol).unwrap_or(u16::MAX);
                offsets[usize::from(l)] += 1;
            }
        }
        Self { counts, symbols }
    }

    /// Codes are stored most significant bit first, one bit at a time.
    fn decode(&self, bits: &mut Bits<'_>) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= i32::try_from(bits.take(1)?).ok()?;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return self.symbols.get(usize::try_from(index + code - first).ok()?).copied();
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// The decompressed bytes of a zlib stream; `None` when it is malformed.
pub(crate) fn zlib(data: &[u8]) -> Option<Vec<u8>> {
    let (&cmf, &flg) = (data.first()?, data.get(1)?);
    if cmf & 0x0F != 8 || flg & 0x20 != 0 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return None;
    }
    inflate(&data[2..])
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut bits = Bits { data, pos: 0, buf: 0, count: 0 };
    let mut out = Vec::new();
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => {
                bits.align();
                let header = data.get(bits.pos..bits.pos + 4)?;
                let len = usize::from(u16::from_le_bytes([header[0], header[1]]));
                bits.pos += 4;
                out.extend_from_slice(data.get(bits.pos..bits.pos + len)?);
                bits.pos += len;
            }
            1 => {
                let mut lengths = [0u8; 288];
                lengths[..144].fill(8);
                lengths[144..256].fill(9);
                lengths[256..280].fill(7);
                lengths[280..].fill(8);
                block(&mut bits, &mut out, &Huffman::new(&lengths), &Huffman::new(&[5; 30]))?;
            }
            2 => {
                let (lit, dist) = dynamic(&mut bits)?;
                block(&mut bits, &mut out, &lit, &dist)?;
            }
            _ => return None,
        }
        if last {
            return Some(out);
        }
    }
}

fn dynamic(bits: &mut Bits<'_>) -> Option<(Huffman, Huffman)> {
    let nlen = bits.take(5)? as usize + 257;
    let ndist = bits.take(5)? as usize + 1;
    let ncode = bits.take(4)? as usize + 4;
    let mut clens = [0u8; 19];
    for &i in &CLEN_ORDER[..ncode] {
        clens[i] = u8::try_from(bits.take(3)?).ok()?;
    }
    let clen = Huffman::new(&clens);
    let mut lengths = Vec::with_capacity(nlen + ndist);
    while lengths.len() < nlen + ndist {
        let (value, repeat) = match clen.decode(bits)? {
            s @ 0..=15 => (u8::try_from(s).ok()?, 1),
            16 => (*lengths.last()?, 3 + bits.take(2)?),
            17 => (0, 3 + bits.take(3)?),
            18 => (0, 11 + bits.take(7)?),
            _ => return None,
        };
        lengths.extend(core::iter::repeat_n(value, repeat as usize));
    }
    if lengths.len() != nlen + ndist {
        return None;
    }
    Some((Huffman::new(&lengths[..nlen]), Huffman::new(&lengths[nlen..])))
}

fn block(bits: &mut Bits<'_>, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Option<()> {
    loop {
        let symbol = usize::from(lit.decode(bits)?);
        match symbol {
            0..=255 => out.push(u8::try_from(symbol).ok()?),
            256 => return Some(()),
            _ => {
                let i = symbol - 257;
                let len = usize::from(*LENGTH_BASE.get(i)?) + bits.take(u32::from(LENGTH_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                let back = usize::from(*DIST_BASE.get(d)?) + bits.take(u32::from(DIST_EXTRA[d]))? as usize;
                let start = out.len().checked_sub(back)?;
                for k in 0..len {
                    out.push(out[start + k]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::zlib;

    #[test]
    fn fixed_and_stored_blocks() {
        let fixed = [120, 218, 203, 72, 205, 201, 201, 87, 200, 64, 39, 1, 104, 3, 8, 177];
        assert_eq!(zlib(&fixed).as_deref(), Some(&b"hello hello hello hello"[..]));
        let stored = [120, 1, 1, 12, 0, 243, 255, 115, 116, 111, 114, 101, 100, 32, 98, 108, 111, 99, 107, 31, 128, 4, 189];
        assert_eq!(zlib(&stored).as_deref(), Some(&b"stored block"[..]));
        assert_eq!(zlib(&fixed[..8]), None);
    }
}
