// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! What the overlay reads from the game's SWF files: the embedded fonts, `DefineFont3` tags (code
//! 75), whose glyphs are shapes of straight and quadratic edges on a 1024 em square in twips (20480
//! units per em, y down), with the layout block (ascent, descent, leading, advances, kerning pairs
//! by character code); and a movie's first `DefineShape3` (code 32), the cursor of `cursormenu.swf`.

use super::inflate;

const DEFINE_SHAPE_3: u16 = 32;
const DEFINE_FONT_3: u16 = 75;
const HAS_LAYOUT: u8 = 0x80;
const WIDE_OFFSETS: u8 = 0x08;
const WIDE_CODES: u8 = 0x04;
const BOLD: u8 = 0x01;

/// SWF font units per em.
pub(crate) const UNITS_PER_EM: i32 = 20480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    pub x: i32,
    /// Up from the baseline.
    pub y: i32,
    pub on_curve: bool,
}

pub(crate) struct Glyph {
    pub code: u16,
    pub advance: i32,
    pub contours: Vec<Vec<Point>>,
}

pub(crate) struct Font {
    pub ascent: i32,
    pub descent: i32,
    pub leading: i32,
    /// Sorted by character code.
    pub glyphs: Vec<Glyph>,
    /// Character codes of the pair and the adjustment.
    pub kerning: Vec<(u16, u16, i32)>,
}

/// A `DefineShape3` of solid fills without strokes, as straight segments in twips, y down, curves flattened.
pub(crate) struct Shape {
    /// RGBA, not premultiplied.
    pub fills: Vec<[u8; 4]>,
    pub edges: Vec<Edge>,
}

/// One segment with the fill on either side: 1-based indices into the fills, 0 for none.
pub(crate) struct Edge {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub fill0: usize,
    pub fill1: usize,
}

struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl Bits<'_> {
    fn unsigned(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.bit / 8)?;
            v = v << 1 | u32::from(byte >> (7 - self.bit % 8) & 1);
            self.bit += 1;
        }
        Some(v)
    }

    fn signed(&mut self, n: u32) -> Option<i32> {
        let v = self.unsigned(n)?;
        Some(if n > 0 && v >> (n - 1) & 1 == 1 { i32::try_from(i64::from(v) - (1i64 << n)).ok()? } else { i32::try_from(v).ok()? })
    }

    fn byte_end(&self) -> usize {
        self.bit.div_ceil(8)
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The tag stream of an `FWS` or `CWS` file.
fn body(file: &[u8]) -> Result<Vec<u8>, String> {
    let body = match file.get(..3) {
        Some(b"FWS") => file.get(8..).ok_or("truncated header")?.to_vec(),
        Some(b"CWS") => inflate::zlib(file.get(8..).ok_or("truncated header")?).ok_or("bad zlib stream")?,
        _ => return Err("not an uncompressed or zlib-compressed SWF file".into()),
    };
    let rect_bits = 5 + 4 * u32::from(*body.first().ok_or("empty")? >> 3);
    let start = (rect_bits as usize).div_ceil(8) + 4;
    Ok(body.get(start..).ok_or("truncated header")?.to_vec())
}

/// Each tag's code and body.
fn tags(data: &[u8]) -> Result<Vec<(u16, &[u8])>, String> {
    let mut tags = Vec::new();
    let mut at = 0;
    while at + 2 <= data.len() {
        let head = u16_at(data, at).ok_or("truncated tag")?;
        at += 2;
        let mut len = usize::from(head & 0x3F);
        if len == 0x3F {
            len = u32_at(data, at).ok_or("truncated tag")? as usize;
            at += 4;
        }
        tags.push((head >> 6, data.get(at..at + len).ok_or("truncated tag")?));
        at += len;
    }
    Ok(tags)
}

/// The first non-bold `DefineFont3` named `name`.
pub(crate) fn font(file: &[u8], name: &str) -> Result<Font, String> {
    let data = body(file)?;
    for (code, tag) in tags(&data)? {
        if code != DEFINE_FONT_3 || tag.len() < 5 {
            continue;
        }
        let name_len = usize::from(tag[4]);
        let tag_name = tag.get(5..5 + name_len).ok_or("truncated font name")?;
        let tag_name = tag_name.strip_suffix(&[0]).unwrap_or(tag_name);
        if tag_name == name.as_bytes() && tag[2] & BOLD == 0 {
            return define_font_3(tag).ok_or_else(|| format!("{name}: malformed DefineFont3"));
        }
    }
    Err(format!("no font named {name}"))
}

/// The movie's first `DefineShape3`.
pub(crate) fn first_shape(file: &[u8]) -> Result<Shape, String> {
    let data = body(file)?;
    let tag = tags(&data)?.into_iter().find(|(code, _)| *code == DEFINE_SHAPE_3).ok_or("no DefineShape3")?.1;
    define_shape_3(tag).ok_or_else(|| "malformed DefineShape3, or one with strokes or other than solid fills".to_owned())
}

fn define_shape_3(tag: &[u8]) -> Option<Shape> {
    let mut bits = Bits { data: tag, bit: 16 };
    let n = bits.unsigned(5)?;
    for _ in 0..4 {
        bits.signed(n)?;
    }
    let mut at = bits.byte_end();
    let count = |at: &mut usize| -> Option<usize> {
        let mut n = usize::from(*tag.get(*at)?);
        *at += 1;
        if n == 0xFF {
            n = usize::from(u16_at(tag, *at)?);
            *at += 2;
        }
        Some(n)
    };
    let rgba = |at: usize| -> Option<[u8; 4]> { tag.get(at..at + 4)?.try_into().ok() };
    let mut colors = Vec::new();
    for _ in 0..count(&mut at)? {
        if *tag.get(at)? != 0 {
            return None;
        }
        colors.push(rgba(at + 1)?);
        at += 5;
    }
    at += 6 * count(&mut at)?;
    let mut bits = Bits { data: tag, bit: at * 8 };
    let fill_bits = bits.unsigned(4)?;
    let line_bits = bits.unsigned(4)?;
    let (mut x, mut y) = (0i32, 0i32);
    let (mut fill0, mut fill1) = (0usize, 0usize);
    let mut edges = Vec::new();
    loop {
        if bits.unsigned(1)? == 0 {
            let flags = bits.unsigned(5)?;
            if flags == 0 {
                return Some(Shape { fills: colors, edges });
            }
            if flags & 0x01 != 0 {
                let n = bits.unsigned(5)?;
                x = bits.signed(n)?;
                y = bits.signed(n)?;
            }
            if flags & 0x02 != 0 {
                fill0 = bits.unsigned(fill_bits)? as usize;
            }
            if flags & 0x04 != 0 {
                fill1 = bits.unsigned(fill_bits)? as usize;
            }
            if flags & 0x08 != 0 && bits.unsigned(line_bits)? != 0 {
                return None;
            }
            if flags & 0x10 != 0 {
                return None;
            }
            continue;
        }
        let straight = bits.unsigned(1)? == 1;
        let n = bits.unsigned(4)? + 2;
        #[allow(clippy::cast_precision_loss)]
        let point = |x: i32, y: i32| [x as f32, y as f32];
        let from = point(x, y);
        if straight {
            let general = bits.unsigned(1)? == 1;
            let vertical = !general && bits.unsigned(1)? == 1;
            if general || !vertical {
                x += bits.signed(n)?;
            }
            if general || vertical {
                y += bits.signed(n)?;
            }
            edges.push(Edge { from, to: point(x, y), fill0, fill1 });
        } else {
            let (cx, cy) = (x + bits.signed(n)?, y + bits.signed(n)?);
            x = cx + bits.signed(n)?;
            y = cy + bits.signed(n)?;
            let (control, to) = (point(cx, cy), point(x, y));
            let at_t = |t: f32| core::array::from_fn(|i| (1.0 - t) * (1.0 - t) * from[i] + 2.0 * t * (1.0 - t) * control[i] + t * t * to[i]);
            let mut last = from;
            for step in 1..=8u8 {
                let next = at_t(f32::from(step) / 8.0);
                edges.push(Edge { from: last, to: next, fill0, fill1 });
                last = next;
            }
        }
    }
}

fn define_font_3(tag: &[u8]) -> Option<Font> {
    let flags = tag[2];
    if flags & HAS_LAYOUT == 0 || flags & WIDE_CODES == 0 {
        return None;
    }
    let mut at = 5 + usize::from(tag[4]);
    let count = usize::from(u16_at(tag, at)?);
    at += 2;
    let table = at;
    let offset = |i: usize| if flags & WIDE_OFFSETS != 0 { u32_at(tag, table + 4 * i).map(|v| v as usize) } else { u16_at(tag, table + 2 * i).map(usize::from) };
    let code_table = table + offset(count)?;
    let mut glyphs = Vec::with_capacity(count);
    for i in 0..count {
        let end = if i + 1 < count { table + offset(i + 1)? } else { code_table };
        let contours = shape(tag.get(table + offset(i)?..end)?)?;
        glyphs.push(Glyph { code: u16_at(tag, code_table + 2 * i)?, advance: 0, contours });
    }
    at = code_table + 2 * count;
    let ascent = i32::from(u16_at(tag, at)?);
    let descent = i32::from(u16_at(tag, at + 2)?);
    let leading = i32::from(u16_at(tag, at + 4)?.cast_signed());
    at += 6;
    for glyph in &mut glyphs {
        glyph.advance = i32::from(u16_at(tag, at)?.cast_signed());
        at += 2;
    }
    for _ in 0..count {
        let mut bits = Bits { data: tag.get(at..)?, bit: 0 };
        let n = bits.unsigned(5)?;
        bits.unsigned(4 * n)?;
        at += bits.byte_end();
    }
    let pairs = usize::from(u16_at(tag, at)?);
    at += 2;
    let mut kerning = Vec::with_capacity(pairs);
    for k in 0..pairs {
        let p = at + 6 * k;
        kerning.push((u16_at(tag, p)?, u16_at(tag, p + 2)?, i32::from(u16_at(tag, p + 4)?.cast_signed())));
    }
    glyphs.sort_by_key(|g| g.code);
    Some(Font { ascent, descent, leading, glyphs, kerning })
}

/// A glyph's SHAPE as closed contours, y up; every move starts a contour, the closing point is dropped.
fn shape(data: &[u8]) -> Option<Vec<Vec<Point>>> {
    let mut bits = Bits { data, bit: 0 };
    let fill_bits = bits.unsigned(4)?;
    let line_bits = bits.unsigned(4)?;
    let (mut x, mut y) = (0i32, 0i32);
    let mut contours: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    loop {
        if bits.unsigned(1)? == 0 {
            let flags = bits.unsigned(5)?;
            if flags == 0 {
                close(&mut current, &mut contours);
                return Some(contours);
            }
            if flags & 0x01 != 0 {
                let n = bits.unsigned(5)?;
                x = bits.signed(n)?;
                y = bits.signed(n)?;
                close(&mut current, &mut contours);
                current.push(Point { x, y: -y, on_curve: true });
            }
            if flags & 0x02 != 0 {
                bits.unsigned(fill_bits)?;
            }
            if flags & 0x04 != 0 {
                bits.unsigned(fill_bits)?;
            }
            if flags & 0x08 != 0 {
                bits.unsigned(line_bits)?;
            }
            if flags & 0x10 != 0 {
                return None;
            }
            continue;
        }
        if current.is_empty() {
            current.push(Point { x, y: -y, on_curve: true });
        }
        let straight = bits.unsigned(1)? == 1;
        let n = bits.unsigned(4)? + 2;
        if straight {
            let general = bits.unsigned(1)? == 1;
            let vertical = !general && bits.unsigned(1)? == 1;
            if general || !vertical {
                x += bits.signed(n)?;
            }
            if general || vertical {
                y += bits.signed(n)?;
            }
        } else {
            x += bits.signed(n)?;
            y += bits.signed(n)?;
            current.push(Point { x, y: -y, on_curve: false });
            x += bits.signed(n)?;
            y += bits.signed(n)?;
        }
        current.push(Point { x, y: -y, on_curve: true });
    }
}

fn close(current: &mut Vec<Point>, contours: &mut Vec<Vec<Point>>) {
    if current.len() > 1 && current.first() == current.last() {
        current.pop();
    }
    if current.len() > 2 {
        contours.push(core::mem::take(current));
    }
    current.clear();
}
