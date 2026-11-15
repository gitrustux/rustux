// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! viewer: Rustica's Image Viewer (DESKTOP.md step F7, "next in line"): a
//! PNG or JPEG picture, made smaller to fit the window; Left and Right
//! show the other pictures of its folder.
//!
//!   viewer FILE

use rustica_ui::gfx::{self, Rect};
use rustica_ui::{Control, Ui};
use std::path::PathBuf;

struct Viewer {
    files: Vec<PathBuf>,
    cur: usize,
    image: Option<(usize, Result<gfx::Pixmap, String>)>,
    scaled: Option<((u32, u32), gfx::Pixmap)>,
}

fn is_image(p: &std::path::Path) -> bool {
    matches!(p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg"))
}

impl Viewer {
    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ui.padding = 0.0;
        for k in &ui.input.keys {
            match k.usage {
                0x4F if !self.files.is_empty() => {
                    self.cur = (self.cur + 1) % self.files.len();
                }
                0x50 if !self.files.is_empty() => {
                    self.cur = (self.cur + self.files.len() - 1) % self.files.len();
                }
                0x29 => ctl.quit = true,
                _ => {}
            }
        }
        let Some(path) = self.files.get(self.cur).cloned() else {
            ui.label("No pictures");
            return;
        };
        if self.image.as_ref().map(|(i, _)| *i) != Some(self.cur) {
            self.image = Some((self.cur, gfx::load_image(&path.display().to_string())));
            self.scaled = None;
        }
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (w, h) = (ui.canvas.width(), ui.canvas.height());
        ui.canvas.fill_rect(Rect::new(0.0, 0.0, w, h), 0x1E1F29);
        match &self.image {
            Some((_, Ok(img))) => {
                ctl.requests.push(rustica_ui::Request::Title(format!("{name} ({}×{}) — Image Viewer", img.width(), img.height())));
                let k = (w / img.width() as f32).min(h / img.height() as f32).min(1.0);
                let (sw, sh) = (((img.width() as f32 * k) as u32).max(1), ((img.height() as f32 * k) as u32).max(1));
                if self.scaled.as_ref().map(|(s, _)| *s) != Some((sw, sh)) {
                    let mut p = gfx::Pixmap::new(sw, sh).unwrap();
                    let t = tiny_skia_transform(k);
                    p.draw_pixmap(0, 0, img.as_ref(), &gfx::PixmapPaint { quality: gfx::FilterQuality::Bilinear, ..Default::default() }, t, None);
                    self.scaled = Some(((sw, sh), p));
                }
                if let Some((_, p)) = &self.scaled {
                    ui.canvas.image(p, ((w - sw as f32) / 2.0).round(), ((h - sh as f32) / 2.0).round());
                }
            }
            Some((_, Err(e))) => {
                let e = e.clone();
                ctl.requests.push(rustica_ui::Request::Title(format!("{name} — Image Viewer")));
                ui.label(&format!("{name}: {e}"));
            }
            None => {}
        }
    }
}

fn tiny_skia_transform(k: f32) -> gfx::Transform {
    gfx::Transform::from_scale(k, k)
}

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: viewer FILE");
        std::process::exit(2);
    };
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_image(p)).collect()).unwrap_or_default();
    files.sort();
    let cur = files.iter().position(|p| p.file_name() == path.file_name()).unwrap_or_else(|| {
        files.insert(0, path.clone());
        0
    });
    let mut v = Viewer { files, cur, image: None, scaled: None };
    if let Err(e) = rustica_ui::run("Image Viewer", 800, 600, None, |ui, ctl| v.frame(ui, ctl)) {
        eprintln!("viewer: {e}");
        std::process::exit(1);
    }
}
