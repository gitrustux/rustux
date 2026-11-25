// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Windows on the Rustica compositor from Rust (DESKTOP.md step F4,
//! docs/WINDOWS.md): a safe wrapper of the C library's window client
//! (`<rustux/window.h>`, libc `window.c`), which Rust programs on Rustux
//! link with anyway.
//!
//! ```no_run
//! let mut w = rustica_window::Window::new("hello", 320, 200, None).unwrap();
//! w.pixels().fill(0x282A36);
//! w.commit(&[]);
//! while let Some(ev) = w.next_event(None) { /* ... */ }
//! ```

use std::ffi::CString;
use std::time::Duration;

/// rx_window_rect_t
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// rx_window_event_t
#[repr(C)]
#[derive(Clone, Copy)]
struct RawEvent {
    op: u32,
    status: i32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    code: u32,
    mods: u32,
    flags: u32,
    buttons: u32,
    wheel: i32,
    n: u32,
    text: [u8; 16],
    time: u64,
}

/// rx_window_t
#[repr(C)]
struct RawWindow {
    ch: u32,
    vmo: u32,
    pixels: *mut u32,
    width: u32,
    height: u32,
    focused: i32,
    frame_pending: i32,
    queue: [RawEvent; 64],
    qhead: i32,
    qlen: i32,
}

extern "C" {
    fn rx_window_create(w: *mut RawWindow, x: i32, y: i32, width: u32, height: u32, title: *const u8) -> i32;
    fn rx_window_commit(w: *mut RawWindow, rects: *const Rect, n: i32) -> i32;
    fn rx_window_wait_frame(w: *mut RawWindow, deadline: u64) -> i32;
    fn rx_window_next_event(w: *mut RawWindow, ev: *mut RawEvent, deadline: u64) -> i32;
    fn rx_window_resize(w: *mut RawWindow, width: u32, height: u32) -> i32;
    fn rx_window_set_title(w: *mut RawWindow, title: *const u8) -> i32;
    fn rx_window_set_fullscreen(w: *mut RawWindow, on: i32) -> i32;
    fn rx_window_clipboard_set(w: *mut RawWindow, text: *const u8) -> i32;
    fn rx_window_clipboard_get(w: *mut RawWindow, buf: *mut u8, size: usize) -> i32;
    fn rx_window_destroy(w: *mut RawWindow);
    fn rx_window_set_role(w: *mut RawWindow, role: u32) -> i32;
    fn rx_window_activate(w: *mut RawWindow, id: i32) -> i32;
    fn rx_window_set_panel(w: *mut RawWindow, edge: u32, size: i32, autohide: i32) -> i32;
    fn rx_window_drag_start(w: *mut RawWindow, text: *const u8) -> i32;
    fn rx_window_drop_text(buf: *mut u8, size: usize) -> i32;
    fn rx_window_lock(w: *mut RawWindow) -> i32;
    fn rx_window_notify(w: *mut RawWindow, text: *const u8) -> i32;
    fn rx_window_wake(ch: u32) -> i32;
    fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
}

const TIME_INFINITE: u64 = u64::MAX;

/// Pointer event kinds (flags)
pub const PTR_MOTION: u32 = 0x01;
pub const PTR_BUTTON: u32 = 0x02;
pub const PTR_WHEEL: u32 = 0x04;
pub const PTR_LEAVE: u32 = 0x08;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The compositor wants this size: call [`Window::resize`], draw, commit
    Configure { width: u32, height: u32 },
    /// HID usage, modifiers, pressed (else released), repeat, the layout's text
    Key { usage: u32, mods: u32, pressed: bool, repeat: bool, text: String },
    /// Position in the content, buttons held (bit 0 left), wheel, PTR_* flags
    Pointer { x: i32, y: i32, buttons: u32, wheel: i32, flags: u32 },
    Focus(bool),
    CloseRequest,
    /// Maximised, full screen, minimised
    State { maximized: bool, fullscreen: bool, minimized: bool },
    Frame,
    /// To the panel (step F6): id -1 starts a new list; then one per window
    WindowInfo { id: i32, title: String, focused: bool, minimized: bool },
    /// [`Waker::wake`] was called
    Wake,
    /// The theme or the wallpaper changed; the theme's name
    Appearance { theme: String },
    /// To the panel: the desktop's menu was used (1 change the wallpaper,
    /// 2 the appearance settings)
    Desktop(u32),
    /// A drag ended over this window (step F12): where, whether it came
    /// from this window, its text (file paths, a line each)
    Drop { x: i32, y: i32, same: bool, text: String },
}

/// Wakes a window's event loop from another thread: its next_event
/// returns [`Event::Wake`]
#[derive(Clone, Copy, Debug)]
pub struct Waker(u32);

impl Waker {
    pub fn wake(&self) {
        unsafe { rx_window_wake(self.0) };
    }
}

/// The panel's role (set_role)
pub const ROLE_PANEL: u32 = 1;
pub const ROLE_DESKTOP: u32 = 2;
pub const EDGE_BOTTOM: u32 = 0;
pub const EDGE_TOP: u32 = 1;
pub const EDGE_LEFT: u32 = 2;
pub const EDGE_RIGHT: u32 = 3;

/// A notification on the desktop without a window
pub fn notify(text: &str) -> std::io::Result<()> {
    let t = CString::new(text).unwrap_or_default();
    let r = unsafe { rx_window_notify(std::ptr::null_mut(), t.as_ptr() as *const u8) };
    if r < 0 { Err(errno(r)) } else { Ok(()) }
}

/// A window: one channel to the compositor and a buffer of pixels
/// (0x00RRGGBB, `width()` pixels a row)
pub struct Window {
    raw: Box<RawWindow>,
    gone: bool,
}

fn errno(r: i32) -> std::io::Error {
    std::io::Error::from_raw_os_error(-r)
}

fn deadline(timeout: Option<Duration>) -> u64 {
    match timeout {
        None => TIME_INFINITE,
        Some(d) => {
            // The kernel's monotonic clock, which deadlines are in (CLOCK_MONOTONIC)
            let mut ts = [0i64; 2];
            unsafe { clock_gettime(1, &mut ts) };
            ((ts[0] as u64) * 1_000_000_000 + ts[1] as u64).saturating_add(d.as_nanos() as u64)
        }
    }
}

impl Window {
    /// A window of width x height pixels, at `at` on the screen or where
    /// the compositor chooses
    pub fn new(title: &str, width: u32, height: u32, at: Option<(i32, i32)>) -> std::io::Result<Window> {
        // The C library fills it in; it starts zeroed
        let mut raw: Box<RawWindow> = unsafe { Box::new(std::mem::zeroed()) };
        let t = CString::new(title).unwrap_or_default();
        let (x, y) = at.unwrap_or((-1, -1));
        let r = unsafe { rx_window_create(&mut *raw, x, y, width, height, t.as_ptr() as *const u8) };
        if r < 0 {
            return Err(errno(r));
        }
        Ok(Window { raw, gone: false })
    }

    /// Something that wakes this window's event loop from another thread
    pub fn waker(&self) -> Waker {
        Waker(self.raw.ch)
    }

    /// False once the compositor is gone (next_event said so)
    pub fn is_alive(&self) -> bool {
        !self.gone
    }

    pub fn width(&self) -> u32 {
        self.raw.width
    }

    pub fn height(&self) -> u32 {
        self.raw.height
    }

    pub fn focused(&self) -> bool {
        self.raw.focused != 0
    }

    pub fn frame_pending(&self) -> bool {
        self.raw.frame_pending != 0
    }

    /// The buffer to draw into
    pub fn pixels(&mut self) -> &mut [u32] {
        let n = self.raw.width as usize * self.raw.height as usize;
        unsafe { std::slice::from_raw_parts_mut(self.raw.pixels, n) }
    }

    /// Show these rectangles (none: everything)
    pub fn commit(&mut self, rects: &[Rect]) {
        unsafe { rx_window_commit(&mut *self.raw, rects.as_ptr(), rects.len() as i32) };
    }

    /// Wait until the last commit is on the screen; false on timeout or
    /// if the compositor is gone
    pub fn wait_frame(&mut self, timeout: Option<Duration>) -> bool {
        unsafe { rx_window_wait_frame(&mut *self.raw, deadline(timeout)) > 0 }
    }

    /// The next event, waiting up to `timeout` (None: as long as it takes);
    /// None on timeout or when the compositor is gone
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Option<Event> {
        let mut ev: RawEvent = unsafe { std::mem::zeroed() };
        let r = unsafe { rx_window_next_event(&mut *self.raw, &mut ev, deadline(timeout)) };
        if r < 0 {
            self.gone = true;
        }
        if r <= 0 {
            return None;
        }
        const CONFIGURE: u32 = 101;
        const KEY: u32 = 102;
        const POINTER: u32 = 103;
        const FOCUS: u32 = 104;
        const CLOSE: u32 = 105;
        const FRAME: u32 = 106;
        const STATE: u32 = 107;
        const WINDOW_INFO: u32 = 108;
        const WAKE: u32 = 109;
        const APPEARANCE: u32 = 110;
        const DESKTOP: u32 = 111;
        const DROP: u32 = 112;
        Some(match ev.op {
            CONFIGURE => Event::Configure { width: ev.w, height: ev.h },
            KEY => {
                let len = ev.text.iter().position(|&b| b == 0).unwrap_or(ev.text.len());
                Event::Key {
                    usage: ev.code,
                    mods: ev.mods,
                    pressed: ev.flags & 1 != 0,
                    repeat: ev.flags & 4 != 0,
                    text: String::from_utf8_lossy(&ev.text[..len]).into_owned(),
                }
            }
            POINTER => Event::Pointer { x: ev.x, y: ev.y, buttons: ev.buttons, wheel: ev.wheel, flags: ev.flags },
            FOCUS => Event::Focus(ev.flags != 0),
            CLOSE => Event::CloseRequest,
            STATE => Event::State {
                maximized: ev.flags & 1 != 0,
                fullscreen: ev.flags & 2 != 0,
                minimized: ev.flags & 4 != 0,
            },
            FRAME => Event::Frame,
            WAKE => Event::Wake,
            APPEARANCE => {
                let len = ev.text.iter().position(|&b| b == 0).unwrap_or(ev.text.len());
                Event::Appearance { theme: String::from_utf8_lossy(&ev.text[..len]).into_owned() }
            }
            DESKTOP => Event::Desktop(ev.flags),
            DROP => {
                let mut buf = vec![0u8; 4097];
                let n = unsafe { rx_window_drop_text(buf.as_mut_ptr(), buf.len()) }.max(0) as usize;
                buf.truncate(n.min(4096));
                Event::Drop { x: ev.x, y: ev.y, same: ev.flags & 1 != 0, text: String::from_utf8_lossy(&buf).into_owned() }
            }
            WINDOW_INFO => {
                let len = ev.text.iter().position(|&b| b == 0).unwrap_or(ev.text.len());
                Event::WindowInfo {
                    id: ev.x,
                    title: String::from_utf8_lossy(&ev.text[..len]).into_owned(),
                    focused: ev.flags & 1 != 0,
                    minimized: ev.flags & 2 != 0,
                }
            }
            _ => return self.next_event(timeout),
        })
    }

    /// A new buffer of width x height (after Configure)
    pub fn resize(&mut self, width: u32, height: u32) -> std::io::Result<()> {
        let r = unsafe { rx_window_resize(&mut *self.raw, width, height) };
        if r < 0 { Err(errno(r)) } else { Ok(()) }
    }

    pub fn set_title(&mut self, title: &str) {
        let t = CString::new(title).unwrap_or_default();
        unsafe { rx_window_set_title(&mut *self.raw, t.as_ptr() as *const u8) };
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        unsafe { rx_window_set_fullscreen(&mut *self.raw, on as i32) };
    }

    /// The desktop session (step F6): become the panel
    pub fn set_role(&mut self, role: u32) {
        unsafe { rx_window_set_role(&mut *self.raw, role) };
    }

    /// Start dragging `text` (file paths, a line each) while the left
    /// button is held; the window under the pointer gets [`Event::Drop`]
    pub fn drag_start(&mut self, text: &str) {
        let t = CString::new(text).unwrap_or_default();
        unsafe { rx_window_drag_start(&mut *self.raw, t.as_ptr() as *const u8) };
    }

    /// (The panel) dock at an edge (EDGE_*), this thick, maybe hidden
    /// until the pointer reaches the edge
    pub fn set_panel(&mut self, edge: u32, size: i32, autohide: bool) {
        unsafe { rx_window_set_panel(&mut *self.raw, edge, size, autohide as i32) };
    }

    /// (The panel) focus and raise a window, or minimise it if focused
    pub fn activate(&mut self, id: i32) {
        unsafe { rx_window_activate(&mut *self.raw, id) };
    }

    /// Lock the screen
    pub fn lock(&mut self) {
        unsafe { rx_window_lock(&mut *self.raw) };
    }

    pub fn notify(&mut self, text: &str) {
        let t = CString::new(text).unwrap_or_default();
        unsafe { rx_window_notify(&mut *self.raw, t.as_ptr() as *const u8) };
    }

    pub fn set_clipboard(&mut self, text: &str) {
        let t = CString::new(text).unwrap_or_default();
        unsafe { rx_window_clipboard_set(&mut *self.raw, t.as_ptr() as *const u8) };
    }

    pub fn clipboard(&mut self) -> Option<String> {
        let mut buf = vec![0u8; 4001];
        let n = unsafe { rx_window_clipboard_get(&mut *self.raw, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            return None;
        }
        buf.truncate((n as usize).min(4000));
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { rx_window_destroy(&mut *self.raw) };
    }
}
