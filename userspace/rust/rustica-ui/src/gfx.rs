// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Drawing: a canvas over a tiny-skia pixmap (anti-aliased shapes), text
//! with fontdue (anti-aliased glyphs, kerning, a glyph cache), images
//! (PNG through tiny-skia, JPEG through zune-jpeg) and a clip rectangle.
//! Coordinates are in pixels; the UI scales its own units.

use std::collections::HashMap;
pub use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Rect as SkRect};

/// A colour, 0xRRGGBB
pub type Color = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
    pub fn shrink(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
}

/// A font face from a TrueType/OpenType file
pub struct Face {
    font: fontdue::Font,
    cache: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Face {
    pub fn from_bytes(data: &[u8]) -> Result<Face, String> {
        let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).map_err(|e| e.to_string())?;
        Ok(Face { font, cache: HashMap::new() })
    }

    pub fn load(path: &str) -> Result<Face, String> {
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Face::from_bytes(&data)
    }

    fn glyph(&mut self, c: char, px: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (c, (px * 4.0) as u32);
        let font = &self.font;
        self.cache.entry(key).or_insert_with(|| font.rasterize(c, px))
    }

    /// Ascent and descent (positive down) and the line height
    pub fn line(&self, px: f32) -> (f32, f32, f32) {
        match self.font.horizontal_line_metrics(px) {
            Some(m) => (m.ascent, -m.descent, m.new_line_size),
            None => (px * 0.8, px * 0.2, px * 1.2),
        }
    }

    /// The advance of each character, with kerning
    pub fn advances(&mut self, text: &str, px: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(text.len());
        let mut prev: Option<char> = None;
        for c in text.chars() {
            let mut adv = self.glyph(c, px).0.advance_width;
            if let Some(p) = prev {
                if let Some(k) = self.font.horizontal_kern(p, c, px) {
                    // The kern belongs between p and c: add it to p's advance
                    if let Some(last) = out.last_mut() {
                        *last += k;
                    }
                }
            }
            if c == '\t' {
                adv = self.glyph(' ', px).0.advance_width * 4.0;
            }
            out.push(adv);
            prev = Some(c);
        }
        out
    }

    pub fn width(&mut self, text: &str, px: f32) -> f32 {
        self.advances(text, px).iter().sum()
    }
}

/// Where drawing goes
pub struct Canvas {
    pub pixmap: Pixmap,
    clip: Option<(Rect, Mask)>,
    clips: Vec<Rect>,
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255);
    p.anti_alias = true;
    p
}

pub fn rounded_path(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    if rad < 0.5 {
        pb.push_rect(SkRect::from_xywh(r.x, r.y, r.w, r.h)?);
        return pb.finish();
    }
    let k = 0.5523 * rad; // cubic approximation of a quarter circle
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - rad + k, y0, x1, y0 + rad - k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - rad + k, x1 - rad + k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + rad - k, y1, x0, y1 - rad + k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + rad - k, x0 + rad - k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { pixmap: Pixmap::new(w.max(1), h.max(1)).unwrap(), clip: None, clips: Vec::new() }
    }

    pub fn width(&self) -> f32 {
        self.pixmap.width() as f32
    }

    pub fn height(&self) -> f32 {
        self.pixmap.height() as f32
    }

    pub fn clear(&mut self, c: Color) {
        self.pixmap.fill(tiny_skia::Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255));
    }

    fn current_clip(&self) -> Rect {
        self.clips.last().copied().unwrap_or(Rect::new(0.0, 0.0, self.width(), self.height()))
    }

    fn set_mask(&mut self) {
        let r = self.current_clip();
        if self.clips.is_empty() {
            self.clip = None;
            return;
        }
        let mut m = Mask::new(self.pixmap.width(), self.pixmap.height()).unwrap();
        if let Some(rect) = SkRect::from_xywh(r.x.floor(), r.y.floor(), r.w.ceil().max(1.0), r.h.ceil().max(1.0)) {
            let path = PathBuilder::from_rect(rect);
            m.fill_path(&path, FillRule::Winding, false, Transform::identity());
        }
        self.clip = Some((r, m));
    }

    /// Draw only inside r (and the clip already set) until pop_clip
    pub fn push_clip(&mut self, r: Rect) {
        let r = r.intersect(&self.current_clip());
        self.clips.push(r);
        self.set_mask();
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
        self.set_mask();
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        // Pixel-aligned rectangles straight into the pixels (most of the UI)
        let cl = r.intersect(&self.current_clip());
        if cl.w <= 0.0 || cl.h <= 0.0 {
            return;
        }
        if r.x.fract() == 0.0 && r.y.fract() == 0.0 && r.w.fract() == 0.0 && r.h.fract() == 0.0 {
            let w = self.pixmap.width() as usize;
            let (x0, y0) = (cl.x.ceil() as usize, cl.y.ceil() as usize);
            let (x1, y1) = (cl.right().floor() as usize, cl.bottom().floor() as usize);
            let px = [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255];
            let data = self.pixmap.data_mut();
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    data[i..i + 4].copy_from_slice(&px);
                }
            }
            return;
        }
        if let Some(rect) = SkRect::from_xywh(r.x, r.y, r.w, r.h) {
            let p = paint(c);
            self.pixmap.fill_rect(rect, &p, Transform::identity(), self.clip.as_ref().map(|(_, m)| m));
        }
    }

    pub fn fill_rounded(&mut self, r: Rect, radius: f32, c: Color) {
        if let Some(path) = rounded_path(r, radius) {
            let p = paint(c);
            self.pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), self.clip.as_ref().map(|(_, m)| m));
        }
    }

    pub fn stroke_rounded(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        let half = width / 2.0;
        if let Some(path) = rounded_path(Rect::new(r.x + half, r.y + half, r.w - width, r.h - width), radius) {
            let p = paint(c);
            let stroke = tiny_skia::Stroke { width, ..Default::default() };
            self.pixmap.stroke_path(&path, &p, &stroke, Transform::identity(), self.clip.as_ref().map(|(_, m)| m));
        }
    }

    pub fn fill_path(&mut self, path: &tiny_skia::Path, c: Color) {
        let p = paint(c);
        self.pixmap.fill_path(path, &p, FillRule::Winding, Transform::identity(), self.clip.as_ref().map(|(_, m)| m));
    }

    /// A path filled even-odd (its inner shapes cut out)
    pub fn fill_path_evenodd(&mut self, path: &tiny_skia::Path, c: Color) {
        let p = paint(c);
        self.pixmap.fill_path(path, &p, FillRule::EvenOdd, Transform::identity(), self.clip.as_ref().map(|(_, m)| m));
    }

    /// Text with its top at y; returns the width drawn
    pub fn text(&mut self, face: &mut Face, x: f32, y: f32, text: &str, px: f32, c: Color) -> f32 {
        let (ascent, _, _) = face.line(px);
        let baseline = (y + ascent).round();
        let clip = self.current_clip();
        let advances = face.advances(text, px);
        let w = self.pixmap.width() as i32;
        let h = self.pixmap.height() as i32;
        let (cr, cg, cb) = ((c >> 16) as u8 as u32, (c >> 8) as u8 as u32, c as u8 as u32);
        let mut pen = x;
        for (ch, adv) in text.chars().zip(advances.iter()) {
            if pen > clip.right() {
                break;
            }
            let (m, bitmap) = face.glyph(ch, px);
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (baseline - m.ymin as f32 - m.height as f32) as i32;
            let data = self.pixmap.data_mut();
            for row in 0..m.height as i32 {
                let py = gy + row;
                if py < 0 || py >= h || (py as f32) < clip.y || (py as f32) >= clip.bottom() {
                    continue;
                }
                for col in 0..m.width as i32 {
                    let pxx = gx + col;
                    if pxx < 0 || pxx >= w || (pxx as f32) < clip.x || (pxx as f32) >= clip.right() {
                        continue;
                    }
                    let a = bitmap[(row * m.width as i32 + col) as usize] as u32;
                    if a == 0 {
                        continue;
                    }
                    let i = ((py * w + pxx) * 4) as usize;
                    let blend = |dst: u8, src: u32| ((src * a + dst as u32 * (255 - a)) / 255) as u8;
                    data[i] = blend(data[i], cr);
                    data[i + 1] = blend(data[i + 1], cg);
                    data[i + 2] = blend(data[i + 2], cb);
                    data[i + 3] = 255;
                }
            }
            pen += adv;
        }
        pen - x
    }

    pub fn image(&mut self, img: &Pixmap, x: f32, y: f32) {
        self.pixmap.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            img.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            self.clip.as_ref().map(|(_, m)| m),
        );
    }

    /// The pixels as premultiplied 0xAARRGGBB (the desktop role's buffer)
    pub fn copy_to_alpha(&self, out: &mut [u32]) {
        for (o, p) in out.iter_mut().zip(self.pixmap.data().chunks_exact(4)) {
            *o = (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        }
    }

    /// The pixels as 0x00RRGGBB, for a window buffer of the same size
    pub fn copy_to(&self, out: &mut [u32]) {
        for (o, p) in out.iter_mut().zip(self.pixmap.data().chunks_exact(4)) {
            *o = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        }
    }
}

/// An image from a PNG, JPEG or QOI file
pub fn load_image(path: &str) -> Result<Pixmap, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    decode_image(&data)
}

/// A picture scaled to `size` x `size` pixels, smoothly (icons)
pub fn scale_square(img: &Pixmap, size: u32) -> Option<Pixmap> {
    let size = size.max(1);
    let mut out = Pixmap::new(size, size)?;
    let sx = size as f32 / img.width() as f32;
    let sy = size as f32 / img.height() as f32;
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..PixmapPaint::default() };
    out.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_scale(sx, sy), None);
    Some(out)
}

pub fn decode_image(data: &[u8]) -> Result<Pixmap, String> {
    if data.starts_with(b"\x89PNG") {
        return Pixmap::decode_png(data).map_err(|e| e.to_string());
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        let mut dec = zune_jpeg::JpegDecoder::new(data);
        let pixels = dec.decode().map_err(|e| format!("{e:?}"))?;
        let info = dec.info().ok_or("no JPEG header")?;
        let (w, h) = (info.width as u32, info.height as u32);
        let comps = pixels.len() / (w as usize * h as usize).max(1);
        let mut pm = Pixmap::new(w, h).ok_or("bad size")?;
        for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
            let s = &pixels[i * comps..];
            if comps >= 3 {
                px.copy_from_slice(&[s[0], s[1], s[2], 255]);
            } else {
                px.copy_from_slice(&[s[0], s[0], s[0], 255]);
            }
        }
        return Ok(pm);
    }
    if data.starts_with(b"qoif") {
        return decode_qoi(data);
    }
    Err("not a PNG, JPEG or QOI image".into())
}

/// A QOI image (qoiformat.org: the wallpapers' format, which the
/// compositor reads); alpha is kept as it is
pub fn decode_qoi(data: &[u8]) -> Result<Pixmap, String> {
    if data.len() < 22 || !data.starts_with(b"qoif") {
        return Err("not a QOI image".into());
    }
    let w = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let h = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err("bad QOI size".into());
    }
    let mut pm = Pixmap::new(w, h).ok_or("bad size")?;
    let (mut idx, mut c, mut run, mut p) = ([[0u8; 4]; 64], [0u8, 0, 0, 255], 0u32, 14usize);
    let end = data.len() - 8;
    for px in pm.data_mut().chunks_exact_mut(4) {
        if run > 0 {
            run -= 1;
        } else if p < end {
            let b1 = data[p];
            p += 1;
            match b1 {
                0xFE if p + 3 <= end => {
                    c[..3].copy_from_slice(&data[p..p + 3]);
                    p += 3;
                }
                0xFF if p + 4 <= end => {
                    c.copy_from_slice(&data[p..p + 4]);
                    p += 4;
                }
                _ if b1 >> 6 == 0 => c = idx[b1 as usize],
                _ if b1 >> 6 == 1 => {
                    c[0] = c[0].wrapping_add((b1 >> 4) & 3).wrapping_sub(2);
                    c[1] = c[1].wrapping_add((b1 >> 2) & 3).wrapping_sub(2);
                    c[2] = c[2].wrapping_add(b1 & 3).wrapping_sub(2);
                }
                _ if b1 >> 6 == 2 && p < end => {
                    let b2 = data[p];
                    p += 1;
                    let vg = (b1 & 0x3F).wrapping_sub(32);
                    c[0] = c[0].wrapping_add(vg.wrapping_sub(8).wrapping_add(b2 >> 4));
                    c[1] = c[1].wrapping_add(vg);
                    c[2] = c[2].wrapping_add(vg.wrapping_sub(8).wrapping_add(b2 & 0xF));
                }
                _ if b1 >> 6 == 3 => run = (b1 & 0x3F) as u32,
                _ => {}
            }
            let hsh = (c[0] as usize * 3 + c[1] as usize * 5 + c[2] as usize * 7 + c[3] as usize * 11) % 64;
            idx[hsh] = c;
        }
        // tiny-skia keeps premultiplied colours
        let a = c[3] as u32;
        px.copy_from_slice(&[(c[0] as u32 * a / 255) as u8, (c[1] as u32 * a / 255) as u8, (c[2] as u32 * a / 255) as u8, c[3]]);
    }
    Ok(pm)
}

/// A pixmap as a QOI file (opaque: RGB)
pub fn encode_qoi(pm: &Pixmap) -> Vec<u8> {
    let (w, h) = (pm.width(), pm.height());
    let mut out = Vec::with_capacity((w * h) as usize + 22);
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&[3, 0]);
    let (mut idx, mut prev, mut run) = ([[0u8; 4]; 64], [0u8, 0, 0, 255], 0u8);
    let px: Vec<[u8; 4]> = pm.pixels().iter().map(|p| { let c = p.demultiply(); [c.red(), c.green(), c.blue(), 255] }).collect();
    let n = px.len();
    for (i, &c) in px.iter().enumerate() {
        if c == prev {
            run += 1;
            if run == 62 || i == n - 1 {
                out.push(0xC0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xC0 | (run - 1));
            run = 0;
        }
        let hsh = (c[0] as usize * 3 + c[1] as usize * 5 + c[2] as usize * 7 + c[3] as usize * 11) % 64;
        if idx[hsh] == c {
            out.push(hsh as u8);
        } else {
            idx[hsh] = c;
            let dr = c[0].wrapping_sub(prev[0]) as i8;
            let dg = c[1].wrapping_sub(prev[1]) as i8;
            let db = c[2].wrapping_sub(prev[2]) as i8;
            let (drg, dbg) = (dr.wrapping_sub(dg), db.wrapping_sub(dg));
            if (-2..=1).contains(&dr) && (-2..=1).contains(&dg) && (-2..=1).contains(&db) {
                out.push(0x40 | ((dr + 2) as u8) << 4 | ((dg + 2) as u8) << 2 | (db + 2) as u8);
            } else if (-32..=31).contains(&dg) && (-8..=7).contains(&drg) && (-8..=7).contains(&dbg) {
                out.push(0x80 | (dg + 32) as u8);
                out.push(((drg + 8) as u8) << 4 | (dbg + 8) as u8);
            } else {
                out.extend_from_slice(&[0xFE, c[0], c[1], c[2]]);
            }
        }
        prev = c;
    }
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out
}
