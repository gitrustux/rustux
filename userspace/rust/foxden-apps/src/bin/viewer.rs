// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! viewer: Rustica's Image Viewer (DESKTOP.md step F7, "next in line";
//! an app since FoxDen step FD6b, org.rustica.imageviewer): a
//! PNG or JPEG picture, made smaller to fit the window; Left and Right
//! show the other pictures of its folder that it can see (opened from
//! Files in its sandbox: the one picture, appctl open, unless it has the
//! files: permission for that folder).
//!
//! Started without a file (from the menu), or with File > Open… (Ctrl+O),
//! it asks the user with the file chooser (<rustux/portal.h>, as the Text
//! Editor) and shows the picture chosen.
//!
//!   viewer [FILE]

use rustica_ui::gfx::{self, Rect};
use rustica_ui::{Control, Ui};
use std::ffi::{c_char, CStr, CString};
use std::io::Read;
use std::os::fd::FromRawFd;
use std::path::PathBuf;

extern "C" {
    fn rx_portal_open(title: *const c_char, patterns: *const c_char, name: *mut c_char, cap: usize) -> i32;
}

const ENOSYS: i32 = 38;
const ECANCELED: i32 = 125;

/// A picture the user chooses: its name and what it decodes to (Err(None):
/// the user said no)
fn choose() -> Result<(String, Result<gfx::Pixmap, String>), Option<String>> {
    let t = CString::new("Open a picture").unwrap();
    let x = CString::new("*.png *.jpg *.jpeg *.qoi").unwrap();
    let mut name = [0 as c_char; 256];
    let fd = unsafe { rx_portal_open(t.as_ptr(), x.as_ptr(), name.as_mut_ptr(), name.len()) };
    if fd < 0 {
        return match -fd {
            ECANCELED => Err(None),
            ENOSYS => Err(Some("Open: start the Image Viewer from the menu or Files".into())),
            e => Err(Some(format!("Open: {}", std::io::Error::from_raw_os_error(e)))),
        };
    }
    let n = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned();
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut data = Vec::new();
    let img = match f.read_to_end(&mut data) {
        Ok(_) => gfx::decode_image(&data),
        Err(e) => Err(e.to_string()),
    };
    println!("viewer: chosen {n}");
    Ok((n, img))
}

struct Viewer {
    files: Vec<PathBuf>,
    cur: usize,
    image: Option<(usize, Result<gfx::Pixmap, String>)>,
    scaled: Option<((u32, u32), gfx::Pixmap)>,
    status: String,
    /// Ask for a picture at the first frame (started without one)
    ask: bool,
}

fn is_image(p: &std::path::Path) -> bool {
    matches!(p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg"))
}

impl Viewer {
    fn open(&mut self) {
        match choose() {
            Ok((n, img)) => {
                if let Ok(i) = &img {
                    println!("viewer: showing {n} ({}x{})", i.width(), i.height());
                }
                let _ = std::io::Write::flush(&mut std::io::stdout());
                self.files = vec![PathBuf::from(&n)];
                self.cur = 0;
                self.image = Some((0, img));
                self.scaled = None;
                self.status.clear();
            }
            Err(Some(e)) => self.status = e,
            Err(None) => {}
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ui.padding = 0.0;
        if self.ask {
            self.ask = false;
            self.open();
        }
        let menus: [(&str, &[&str]); 1] = [("File", &["Open…", "Quit"])];
        match ui.menu_bar(&menus) {
            Some((0, 0)) => self.open(),
            Some((0, 1)) => ctl.quit = true,
            _ => {}
        }
        let mut open = false;
        for k in &ui.input.keys {
            if k.ctrl() && k.usage == 0x12 {
                open = true; // Ctrl+O
                continue;
            }
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
        if open {
            self.open();
        }
        let Some(path) = self.files.get(self.cur).cloned() else {
            ui.space(40.0);
            ui.label("No picture is open.");
            if ui.primary_button("Open…") {
                self.open();
            }
            if !self.status.is_empty() {
                ui.dim_label(&self.status.clone());
            }
            return;
        };
        if self.image.as_ref().map(|(i, _)| *i) != Some(self.cur) {
            let img = gfx::load_image(&path.display().to_string());
            match &img {
                Ok(i) => println!("viewer: showing {} ({}x{}), {} of {} it can see", path.display(), i.width(), i.height(), self.cur + 1, self.files.len()),
                Err(e) => println!("viewer: {}: {e}", path.display()),
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
            self.image = Some((self.cur, img));
            self.scaled = None;
        }
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // Below the menu bar
        let top = ui.canvas.height() - ui.remaining_height();
        let (w, h) = (ui.canvas.width(), ui.remaining_height());
        ui.canvas.fill_rect(Rect::new(0.0, top, w, h), ui.theme.background);
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
                    ui.canvas.image(p, ((w - sw as f32) / 2.0).round(), (top + (h - sh as f32) / 2.0).round());
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
        // No file: the chooser at once
        let mut v = Viewer { files: Vec::new(), cur: 0, image: None, scaled: None, status: String::new(), ask: true };
        if let Err(e) = rustica_ui::run("Image Viewer", 800, 600, None, |ui, ctl| v.frame(ui, ctl)) {
            eprintln!("viewer: {e}");
            std::process::exit(1);
        }
        return;
    };
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_image(p)).collect()).unwrap_or_default();
    files.sort();
    let cur = files.iter().position(|p| p.file_name() == path.file_name()).unwrap_or_else(|| {
        files.insert(0, path.clone());
        0
    });
    let mut v = Viewer { files, cur, image: None, scaled: None, status: String::new(), ask: false };
    if let Err(e) = rustica_ui::run("Image Viewer", 800, 600, None, |ui, ctl| v.frame(ui, ctl)) {
        eprintln!("viewer: {e}");
        std::process::exit(1);
    }
}
