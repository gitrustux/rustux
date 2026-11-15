// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Running a window: events from the compositor become the UI's input,
//! the program's frame function describes the window, and the result is
//! copied into the window's buffer and committed. A frame is drawn after
//! each mouse button change or key (so none is lost) and once more after
//! the rest of the waiting events.

use crate::gfx::Face;
use crate::ui::{Input, Key, Platform, Ui};
use rustica_window::{Event, Window};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub const FONT: &str = "/usr/share/fonts/inter/Inter-Regular.ttf";
pub const FONT_BOLD: &str = "/usr/share/fonts/inter/Inter-SemiBold.ttf";

struct Clipboard(Rc<RefCell<Window>>);

impl Platform for Clipboard {
    fn clipboard_get(&mut self) -> String {
        self.0.borrow_mut().clipboard().unwrap_or_default()
    }
    fn clipboard_set(&mut self, text: &str) {
        self.0.borrow_mut().set_clipboard(text);
    }
}

/// Something for the window system, asked for by the frame function
pub enum Request {
    /// Become the panel (rustica_window::ROLE_PANEL)
    SetRole(u32),
    /// (The panel) activate a window
    Activate(i32),
    Lock,
    Notify(String),
    Fullscreen(bool),
    /// The window's title (sent only when it changes)
    Title(String),
}

/// What the frame function asks for, and what it is told
#[derive(Default)]
pub struct Control {
    /// End the program after this frame
    pub quit: bool,
    /// Draw a frame again after this long even without input (a clock)
    pub wake_after: Option<Duration>,
    /// Requests, carried out after the frame
    pub requests: Vec<Request>,
    /// Events other than input since the last frame (window lists, focus,
    /// state); the frame function takes what it wants
    pub events: Vec<Event>,
    /// Wakes the loop from another thread (a frame is drawn)
    pub waker: Option<rustica_window::Waker>,
}

/// The scale: RUSTICA_SCALE=2 doubles everything; else the user's
/// setting (scale= in ~/.config/rustica.conf, which Settings writes); 1
/// otherwise
pub fn scale_from_env() -> f32 {
    let conf = || -> Option<String> {
        let home = std::env::var("HOME").ok()?;
        let text = std::fs::read_to_string(format!("{home}/.config/rustica.conf")).ok()?;
        text.lines().find_map(|l| l.trim().strip_prefix("scale=").map(|v| v.trim().to_string()))
    };
    match std::env::var("RUSTICA_SCALE").ok().or_else(conf).and_then(|s| s.parse::<f32>().ok()) {
        Some(s) if (1.0..=4.0).contains(&s) => s.round(),
        _ => 1.0,
    }
}

/// Open a window of w x h units and run frame until it asks to quit or
/// the window is closed. Without a position, RUSTICA_WINDOW_AT=X,Y (for
/// scripts and tests) gives one, else the compositor chooses.
pub fn run(title: &str, w: u32, h: u32, at: Option<(i32, i32)>, mut frame: impl FnMut(&mut Ui, &mut Control)) -> Result<(), String> {
    let scale = scale_from_env();
    let at = at.or_else(|| {
        let v = std::env::var("RUSTICA_WINDOW_AT").ok()?;
        let (x, y) = v.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    });
    let face = Face::load(FONT)?;
    let bold = Face::load(FONT_BOLD)?;
    let win = Window::new(title, (w as f32 * scale) as u32, (h as f32 * scale) as u32, at).map_err(|e| format!("no window: {e}"))?;
    let win = Rc::new(RefCell::new(win));
    let mut ui = Ui::new(face, bold, scale, Box::new(Clipboard(win.clone())));
    let mut ctl = Control::default();
    let mut shown_title = title.to_string();
    ctl.waker = Some(win.borrow().waker());
    let mut draw = |ui: &mut Ui, ctl: &mut Control, commit: bool| {
        loop {
            let (ww, wh) = {
                let w = win.borrow();
                (w.width(), w.height())
            };
            ui.begin(ww, wh);
            frame(ui, ctl);
            ui.end();
            // A frame that changed what is shown (a menu opened, the focus
            // moved) runs again so the picture is current
            if !ui.redraw || ctl.quit {
                break;
            }
        }
        for r in ctl.requests.drain(..) {
            let mut w = win.borrow_mut();
            match r {
                Request::SetRole(role) => w.set_role(role),
                Request::Activate(id) => w.activate(id),
                Request::Lock => w.lock(),
                Request::Notify(t) => w.notify(&t),
                Request::Fullscreen(on) => w.set_fullscreen(on),
                Request::Title(t) => {
                    if t != shown_title {
                        w.set_title(&t);
                        shown_title = t;
                    }
                }
            }
        }
        ctl.events.clear();
        if commit {
            let mut w = win.borrow_mut();
            ui.canvas.copy_to(w.pixels());
            w.commit(&[]);
        }
    };
    draw(&mut ui, &mut ctl, true);
    loop {
        if ctl.quit {
            return Ok(());
        }
        // Wait for an event (or the frame function's wake-up), then take
        // all that are waiting
        let wake = ctl.wake_after;
        let first = win.borrow_mut().next_event(wake);
        let Some(first) = first else {
            if wake.is_some() && win.borrow().is_alive() {
                draw(&mut ui, &mut ctl, true);
                continue;
            }
            return Ok(()); // the compositor is gone
        };
        let mut ev = Some(first);
        let mut dirty = false;
        while let Some(e) = ev.take() {
            let mut now = false;
            match e {
                Event::Frame => {}
                Event::Configure { width, height } => {
                    if win.borrow_mut().resize(width, height).is_ok() {
                        dirty = true;
                    }
                }
                Event::Pointer { x, y, buttons, wheel, flags } => {
                    let inp: &mut Input = &mut ui.input;
                    inp.mouse = (x as f32, y as f32);
                    if flags & rustica_window::PTR_LEAVE != 0 {
                        inp.mouse = (-1.0, -1.0);
                    }
                    if buttons & 2 != 0 && !inp.right_held {
                        inp.right_pressed = true;
                        now = true;
                    }
                    inp.right_held = buttons & 2 != 0;
                    let down = buttons & 1 != 0;
                    if down != inp.down {
                        inp.down = down;
                        if down {
                            inp.pressed = true;
                        } else {
                            inp.released = true;
                        }
                        now = true;
                    }
                    inp.wheel += wheel as f32;
                    dirty = true;
                }
                Event::Key { usage, mods, pressed: true, text, .. } => {
                    ui.input.keys.push(Key { usage, mods, text });
                    now = true;
                }
                Event::Key { .. } => {}
                Event::Focus(_) | Event::State { .. } => {
                    ctl.events.push(e);
                    dirty = true;
                }
                Event::WindowInfo { .. } => {
                    ctl.events.push(e);
                    dirty = true;
                }
                Event::Wake => dirty = true,
                Event::CloseRequest => return Ok(()),
            }
            if now {
                draw(&mut ui, &mut ctl, false);
                dirty = true;
                if ctl.quit {
                    return Ok(());
                }
            }
            ev = win.borrow_mut().next_event(Some(Duration::ZERO));
        }
        if dirty {
            draw(&mut ui, &mut ctl, true);
        }
    }
}
