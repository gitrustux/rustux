// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Text Console
//!
//! This module provides a text console implementation using the framebuffer
//! and font rendering.

use crate::drivers::display::framebuffer::{Color, Framebuffer};
use crate::drivers::display::font::SimpleVgaFont;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global text console instance
static mut CONSOLE: Option<TextConsole> = None;
static CONSOLE_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Text console with framebuffer backing
pub struct TextConsole {
    framebuffer: Framebuffer,
    cursor_x: usize,
    cursor_y: usize,
    fg_color: Color,
    bg_color: Color,
    cols: usize,
    rows: usize,
}

impl TextConsole {
    /// Create a new text console
    pub fn new(framebuffer: Framebuffer) -> Self {
        let char_width = SimpleVgaFont::width();
        let char_height = SimpleVgaFont::height();

        let cols = framebuffer.width / char_width;
        let rows = framebuffer.height / char_height;

        Self {
            framebuffer,
            cursor_x: 0,
            cursor_y: 0,
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
            cols,
            rows,
        }
    }

    /// Get the current foreground color
    pub fn fg_color(&self) -> Color {
        self.fg_color
    }

    /// Get the current background color
    pub fn bg_color(&self) -> Color {
        self.bg_color
    }

    /// Set the foreground and background colors
    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.fg_color = fg;
        self.bg_color = bg;
    }

    /// Get the cursor position (column, row)
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Set the cursor position
    pub fn set_cursor(&mut self, x: usize, y: usize) {
        if x < self.cols && y < self.rows {
            self.cursor_x = x;
            self.cursor_y = y;
        }
    }

    /// Clear the console with the background color
    pub fn clear(&mut self) {
        unsafe {
            self.framebuffer.clear(self.bg_color);
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    /// Put a single character at the current cursor position
    pub fn put_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                // Newline - move to next line
                self.cursor_y += 1;
                self.cursor_x = 0;
            }
            b'\r' => {
                // Carriage return - move to start of line
                self.cursor_x = 0;
            }
            b'\t' => {
                // Tab - move to next tab stop (every 8 columns)
                self.cursor_x = (self.cursor_x + 8) & !7;
                if self.cursor_x >= self.cols {
                    self.cursor_x = 0;
                    self.cursor_y += 1;
                }
            }
            b'\x08' => {
                // Backspace - move cursor back and clear character
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                    self.clear_char_at(self.cursor_x, self.cursor_y);
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                    self.cursor_x = self.cols - 1;
                    self.clear_char_at(self.cursor_x, self.cursor_y);
                }
            }
            0x20..=0x7E => {
                // Printable ASCII character
                self.render_char(ch, self.cursor_x, self.cursor_y);
                self.cursor_x += 1;

                // Check for line wrap
                if self.cursor_x >= self.cols {
                    self.cursor_x = 0;
                    self.cursor_y += 1;
                }
            }
            _ => {
                // Other control characters - ignore for now
            }
        }

        // Check for scroll
        if self.cursor_y >= self.rows {
            self.scroll();
            self.cursor_y = self.rows - 1;
        }
    }

    /// Write a string to the console
    pub fn write_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.put_char(b);
        }
    }

    /// Render a single character at the given position
    fn render_char(&mut self, ch: u8, col: usize, row: usize) {
        let char_width = SimpleVgaFont::width();
        let char_height = SimpleVgaFont::height();

        let x = col * char_width;
        let y = row * char_height;

        // Clear the character cell with background color
        unsafe {
            self.framebuffer.fill_rect(
                x,
                y,
                char_width,
                char_height,
                self.bg_color,
            );
        }

        // Render the character pixels
        for py in 0..char_height {
            for px in 0..char_width {
                if SimpleVgaFont::glyph_pixel(ch, px, py) {
                    unsafe {
                        self.framebuffer.put_pixel(x + px, y + py, self.fg_color);
                    }
                }
            }
        }
    }

    /// Clear the character at the given position
    fn clear_char_at(&mut self, col: usize, row: usize) {
        let char_width = SimpleVgaFont::width();
        let char_height = SimpleVgaFont::height();

        let x = col * char_width;
        let y = row * char_height;

        unsafe {
            self.framebuffer.fill_rect(
                x,
                y,
                char_width,
                char_height,
                self.bg_color,
            );
        }
    }

    /// Scroll the console up by one line
    fn scroll(&mut self) {
        unsafe {
            self.framebuffer.scroll(1, SimpleVgaFont::height());
        }

        // Clear the bottom line
        for col in 0..self.cols {
            self.clear_char_at(col, self.rows - 1);
        }
    }

    /// Get the number of columns
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Get the number of rows
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Get a reference to the framebuffer
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// Get a mutable reference to the framebuffer
    pub fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }
}

/// Initialize the global text console
///
/// # Safety
/// This function must be called only once during kernel initialization.
/// It must be called after the framebuffer has been initialized.
pub unsafe fn init(framebuffer: Framebuffer) {
    CONSOLE = Some(TextConsole::new(framebuffer));
    CONSOLE_INITIALIZED.store(true, Ordering::Release);
}

/// Check if the console has been initialized
pub fn is_initialized() -> bool {
    CONSOLE_INITIALIZED.load(Ordering::Acquire)
}

/// Write a string to the console
pub fn write_str(s: &str) {
    unsafe {
        if let Some(ref mut console) = CONSOLE {
            console.write_str(s);
        }
    }
}

/// Write a single character to the console
pub fn put_char(ch: u8) {
    unsafe {
        if let Some(ref mut console) = CONSOLE {
            console.put_char(ch);
        }
    }
}

/// Clear the console
pub fn clear() {
    unsafe {
        if let Some(ref mut console) = CONSOLE {
            console.clear();
        }
    }
}

/// Set the console colors
pub fn set_color(fg: Color, bg: Color) {
    unsafe {
        if let Some(ref mut console) = CONSOLE {
            console.set_color(fg, bg);
        }
    }
}

/// Get the console colors
pub fn get_color() -> (Color, Color) {
    unsafe {
        if let Some(ref console) = CONSOLE {
            (console.fg_color(), console.bg_color())
        } else {
            (Color::WHITE, Color::BLACK)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialized_flag() {
        assert!(!is_initialized());
    }
}
