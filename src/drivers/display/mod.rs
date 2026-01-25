// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Display Drivers
//!
//! This module provides framebuffer and text console support for
//! displaying graphics and text on the screen.

pub mod framebuffer;
pub mod font;
pub mod console;

// Re-exports
pub use framebuffer::{Framebuffer, Color, PixelFormat};
pub use font::{Psf2Font, SimpleVgaFont};
pub use console::{TextConsole, init, write_str, put_char, clear, set_color, get_color, is_initialized};
