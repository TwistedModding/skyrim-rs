// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! An SWF font written as a TrueType file: quadratic `glyf` outlines, `cmap` format 4, horizontal
//! metrics and a format 0 `kern` table, on a 2048-unit em. Glyph 0 is an empty `.notdef`.

use super::swf::{Font, Point, UNITS_PER_EM};

const EM: i32 = 2048;

fn scale(v: i32) -> i16 {
    let scaled = (i64::from(v) * i64::from(EM) + i64::from(UNITS_PER_EM) / 2).div_euclid(i64::from(UNITS_PER_EM));
    i16::try_from(scaled).unwrap_or(if scaled < 0 { i16::MIN } else { i16::MAX })
}

fn len_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

struct Out(Vec<u8>);

impl Out {
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
}

/// `count` entries: (search range, entry selector, range shift) for a binary search table of `unit`-byte records.
fn search(count: u16, unit: u16) -> (u16, u16, u16) {
    let mut power = 1u16;
    let mut log = 0u16;
    while power * 2 <= count {
        power *= 2;
        log += 1;
    }
    (power * unit, log, (count * unit).saturating_sub(power * unit))
}

struct Bounds {
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
}

/// One glyph's `glyf` record (empty for a glyph without contours), its bounds, point and contour counts.
fn glyph(contours: &[Vec<Point>]) -> (Vec<u8>, Option<Bounds>, usize) {
    let points: Vec<(i16, i16, bool)> = contours.iter().flatten().map(|p| (scale(p.x), scale(p.y), p.on_curve)).collect();
    if points.is_empty() {
        return (Vec::new(), None, 0);
    }
    let bounds = Bounds {
        x_min: points.iter().map(|p| p.0).min().unwrap_or(0),
        y_min: points.iter().map(|p| p.1).min().unwrap_or(0),
        x_max: points.iter().map(|p| p.0).max().unwrap_or(0),
        y_max: points.iter().map(|p| p.1).max().unwrap_or(0),
    };
    let mut out = Out(Vec::new());
    out.i16(i16::try_from(contours.len()).unwrap_or(i16::MAX)).i16(bounds.x_min).i16(bounds.y_min).i16(bounds.x_max).i16(bounds.y_max);
    let mut end = 0usize;
    for c in contours {
        end += c.len();
        out.u16(len_u16(end - 1));
    }
    out.u16(0);
    out.0.extend(points.iter().map(|p| u8::from(p.2)));
    let (mut px, mut py) = (0i16, 0i16);
    for p in &points {
        out.i16(p.0.wrapping_sub(px));
        px = p.0;
    }
    for p in &points {
        out.i16(p.1.wrapping_sub(py));
        py = p.1;
    }
    if out.0.len() % 2 == 1 {
        out.0.push(0);
    }
    (out.0, Some(bounds), points.len())
}

/// The font as a TrueType file `egui` can load.
pub(crate) fn build(font: &Font) -> Vec<u8> {
    let count = len_u16(font.glyphs.len() + 1);
    let space = font.glyphs.iter().find(|g| g.code == u16::from(b' ')).map_or(EM / 4, |g| i32::from(scale(g.advance)));
    let mut glyf = Vec::new();
    let mut loca = Out(Vec::new());
    let mut hmtx = Out(Vec::new());
    let (mut max_points, mut max_contours) = (0usize, 0usize);
    let mut all = Bounds { x_min: 0, y_min: 0, x_max: 0, y_max: 0 };
    let mut advance_max = 0u16;
    loca.u32(0).u32(0);
    hmtx.u16(u16::try_from(space).unwrap_or(0)).i16(0);
    for g in &font.glyphs {
        let (record, bounds, points) = glyph(&g.contours);
        glyf.extend_from_slice(&record);
        loca.u32(u32::try_from(glyf.len()).unwrap_or(u32::MAX));
        let advance = u16::try_from(scale(g.advance).max(0)).unwrap_or(0);
        advance_max = advance_max.max(advance);
        hmtx.u16(advance).i16(bounds.as_ref().map_or(0, |b| b.x_min));
        if let Some(b) = bounds {
            all = Bounds { x_min: all.x_min.min(b.x_min), y_min: all.y_min.min(b.y_min), x_max: all.x_max.max(b.x_max), y_max: all.y_max.max(b.y_max) };
        }
        max_points = max_points.max(points);
        max_contours = max_contours.max(g.contours.len());
    }
    let gid = |code: u16| font.glyphs.binary_search_by_key(&code, |g| g.code).ok().map(|i| len_u16(i + 1));

    let mut head = Out(Vec::new());
    head.u32(0x0001_0000).u32(0x0001_0000).u32(0).u32(0x5F0F_3CF5).u16(0x000B).u16(len_u16(usize::try_from(EM).unwrap_or(0)));
    head.u32(0).u32(0).u32(0).u32(0);
    head.i16(all.x_min).i16(all.y_min).i16(all.x_max).i16(all.y_max).u16(0).u16(8).i16(2).i16(1).i16(0);

    let mut hhea = Out(Vec::new());
    hhea.u32(0x0001_0000).i16(scale(font.ascent)).i16(-scale(font.descent)).i16(scale(font.leading)).u16(advance_max);
    hhea.i16(all.x_min).i16(0).i16(all.x_max).i16(1).i16(0).i16(0);
    hhea.i16(0).i16(0).i16(0).i16(0).i16(0).u16(count);

    let mut maxp = Out(Vec::new());
    maxp.u32(0x0001_0000).u16(count).u16(len_u16(max_points)).u16(len_u16(max_contours)).u16(0).u16(0).u16(2);
    for _ in 0..8 {
        maxp.u16(0);
    }

    let codes: Vec<(u16, u16)> = font.glyphs.iter().enumerate().filter(|(_, g)| g.code != 0xFFFF).map(|(i, g)| (g.code, len_u16(i + 1))).collect();
    let segments = len_u16(codes.len() + 1);
    let (range, selector, shift) = search(segments, 2);
    let mut sub = Out(Vec::new());
    sub.u16(4).u16(len_u16(16 + 8 * usize::from(segments))).u16(0).u16(segments * 2).u16(range).u16(selector).u16(shift);
    for (code, _) in &codes {
        sub.u16(*code);
    }
    sub.u16(0xFFFF).u16(0);
    for (code, _) in &codes {
        sub.u16(*code);
    }
    sub.u16(0xFFFF);
    for (code, glyph) in &codes {
        sub.u16(glyph.wrapping_sub(*code));
    }
    sub.u16(1);
    for _ in 0..segments {
        sub.u16(0);
    }
    let mut cmap = Out(Vec::new());
    cmap.u16(0).u16(1).u16(3).u16(1).u32(12);
    cmap.0.extend_from_slice(&sub.0);

    let mut pairs: Vec<(u16, u16, i16)> = font.kerning.iter().filter_map(|&(a, b, v)| Some((gid(a)?, gid(b)?, scale(v)))).collect();
    pairs.sort_unstable();
    pairs.dedup_by_key(|p| (p.0, p.1));
    let (range, selector, shift) = search(len_u16(pairs.len()), 6);
    let mut kern = Out(Vec::new());
    kern.u16(0).u16(1).u16(0).u16(len_u16(14 + 6 * pairs.len())).u16(1).u16(len_u16(pairs.len())).u16(range).u16(selector).u16(shift);
    for (a, b, v) in &pairs {
        kern.u16(*a).u16(*b).i16(*v);
    }

    let mut post = Out(Vec::new());
    post.u32(0x0003_0000).u32(0).i16(-100).i16(50).u32(0).u32(0).u32(0).u32(0).u32(0);

    let tables: [(&[u8; 4], Vec<u8>); 9] =
        [(b"cmap", cmap.0), (b"glyf", glyf), (b"head", head.0), (b"hhea", hhea.0), (b"hmtx", hmtx.0), (b"kern", kern.0), (b"loca", loca.0), (b"maxp", maxp.0), (b"post", post.0)];
    let (range, selector, shift) = search(len_u16(tables.len()), 16);
    let mut out = Out(Vec::new());
    out.u32(0x0001_0000).u16(len_u16(tables.len())).u16(range).u16(selector).u16(shift);
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in &tables {
        let sum = data.chunks(4).fold(0u32, |s, c| {
            let mut word = [0u8; 4];
            word[..c.len()].copy_from_slice(c);
            s.wrapping_add(u32::from_be_bytes(word))
        });
        out.0.extend_from_slice(*tag);
        out.u32(sum).u32(u32::try_from(offset).unwrap_or(u32::MAX)).u32(u32::try_from(data.len()).unwrap_or(u32::MAX));
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in &tables {
        out.0.extend_from_slice(data);
        out.0.resize(out.0.len().next_multiple_of(4), 0);
    }
    out.0
}
