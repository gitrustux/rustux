// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rustica-window: a window from Rust (DESKTOP.md step F4), the
//! `rustica-window` crate's demo. Draws a gradient that follows the
//! window's size and a square where the pointer is, prints its events
//! ("rustica-window: ..."), and closes when asked.
//!
//!   rustica-window [X,Y]

use rustica_window::{Event, Window};

fn draw(w: &mut Window, px: i32, py: i32) {
    let (width, height) = (w.width() as usize, w.height() as usize);
    let p = w.pixels();
    for y in 0..height {
        for x in 0..width {
            let r = (x * 255 / width.max(1)) as u32;
            let b = (y * 255 / height.max(1)) as u32;
            p[y * width + x] = r << 16 | 0x40 << 8 | b;
        }
    }
    for y in py.max(0)..(py + 16).min(height as i32) {
        for x in px.max(0)..(px + 16).min(width as i32) {
            p[y as usize * width + x as usize] = 0xF8F8F2;
        }
    }
}

fn main() {
    let at = std::env::args().nth(1).and_then(|a| {
        let (x, y) = a.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    let mut w = match Window::new("rustica-window (Rust)", 300, 200, at) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("rustica-window: no window: {e}");
            std::process::exit(1);
        }
    };
    println!("rustica-window: window {}x{}", w.width(), w.height());
    let (mut px, mut py) = (-100, -100);
    draw(&mut w, px, py);
    w.commit(&[]);
    while let Some(ev) = w.next_event(None) {
        match ev {
            Event::Frame => continue,
            Event::Configure { width, height } => {
                println!("rustica-window: configure {width}x{height}");
                if w.resize(width, height).is_ok() {
                    draw(&mut w, px, py);
                    w.commit(&[]);
                }
            }
            Event::Pointer { x, y, buttons, .. } => {
                px = x - 8;
                py = y - 8;
                draw(&mut w, px, py);
                w.commit(&[]);
                if buttons != 0 {
                    println!("rustica-window: buttons {buttons:#x} at {x},{y}");
                }
            }
            Event::Key { text, pressed: true, .. } if !text.is_empty() => println!("rustica-window: key {text:?}"),
            Event::Focus(f) => println!("rustica-window: focus {}", if f { "in" } else { "out" }),
            Event::CloseRequest => {
                println!("rustica-window: closing");
                return;
            }
            _ => {}
        }
    }
    println!("rustica-window: the compositor is gone");
}
