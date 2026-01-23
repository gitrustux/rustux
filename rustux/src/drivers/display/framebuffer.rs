// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Framebuffer Driver
//!
//! This module provides framebuffer management for text console output.
//! The framebuffer is typically obtained from UEFI Graphics Output Protocol (GOP).

/// Pixel format supported by the framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// RGB color order (byte 0: blue, byte 1: green, byte 2: red)
    RGB,
    /// BGR color order (byte 0: red, byte 1: green, byte 2: blue)
    BGR,
}

/// RGB color representation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Black color
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };

    /// White color
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };

    /// Create from RGB565 value (16-bit color)
    pub fn from_rgb565(rgb565: u16) -> Self {
        let r = ((rgb565 >> 11) & 0x1F) as u8;
        let g = ((rgb565 >> 5) & 0x3F) as u8;
        let b = (rgb565 & 0x1F) as u8;

        // Scale 5-bit to 8-bit
        let r = (r * 255 + 15) / 31;
        // Scale 6-bit to 8-bit
        let g = (g * 255 + 31) / 63;
        // Scale 5-bit to 8-bit
        let b = (b * 255 + 15) / 31;

        Self { r, g, b }
    }

    /// Convert to RGB565 value (16-bit color)
    pub fn to_rgb565(self) -> u16 {
        let r = (self.r as u16 * 31) / 255;
        let g = (self.g as u16 * 63) / 255;
        let b = (self.b as u16 * 31) / 255;

        (r << 11) | (g << 5) | b
    }

    /// Convert to 32-bit RGBA value
    pub fn to_rgba32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32) | 0xFF000000
    }
}

/// Framebuffer information and management
pub struct Framebuffer {
    /// Base physical address of framebuffer
    pub base_addr: u64,
    /// Width in pixels
    pub width: usize,
    /// Height in pixels
    pub height: usize,
    /// Bytes per line (stride)
    pub pitch: usize,
    /// Bits per pixel
    pub bpp: usize,
    /// Pixel format
    pub format: PixelFormat,
}

impl Framebuffer {
    /// Create a new framebuffer from raw parameters
    pub const fn new(
        base_addr: u64,
        width: usize,
        height: usize,
        pitch: usize,
        bpp: usize,
        format: PixelFormat,
    ) -> Self {
        Self {
            base_addr,
            width,
            height,
            pitch,
            bpp,
            format,
        }
    }

    /// Get the size in bytes of the framebuffer
    pub const fn size(&self) -> usize {
        self.height * self.pitch
    }

    /// Calculate the offset for a given pixel position
    pub const fn pixel_offset(&self, x: usize, y: usize) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y * self.pitch + x * (self.bpp / 8))
    }

    /// Put a single pixel at the given position
    ///
    /// # Safety
    /// The caller must ensure that the framebuffer memory is valid and accessible.
    pub unsafe fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        if let Some(offset) = self.pixel_offset(x, y) {
            let fb_ptr = self.base_addr as *mut u8;

            match (self.bpp, self.format) {
                (16, PixelFormat::RGB) => {
                    // RGB565 format
                    let rgb565 = color.to_rgb565();
                    let ptr = fb_ptr.add(offset) as *mut u16;
                    *ptr = rgb565.to_le();
                }
                (16, PixelFormat::BGR) => {
                    // BGR565 format (same as RGB565 for 16-bit, just different byte order interpretation)
                    let rgb565 = color.to_rgb565();
                    let ptr = fb_ptr.add(offset) as *mut u16;
                    *ptr = rgb565.to_le();
                }
                (24 | 32, PixelFormat::RGB) => {
                    // RGB888 or RGBA8888 format
                    *(fb_ptr.add(offset)) = color.b;
                    *(fb_ptr.add(offset + 1)) = color.g;
                    *(fb_ptr.add(offset + 2)) = color.r;
                    if self.bpp == 32 {
                        *(fb_ptr.add(offset + 3)) = 0xFF; // Alpha
                    }
                }
                (24 | 32, PixelFormat::BGR) => {
                    // BGR888 or BGRA8888 format
                    *(fb_ptr.add(offset)) = color.r;
                    *(fb_ptr.add(offset + 1)) = color.g;
                    *(fb_ptr.add(offset + 2)) = color.b;
                    if self.bpp == 32 {
                        *(fb_ptr.add(offset + 3)) = 0xFF; // Alpha
                    }
                }
                _ => {
                    // Unsupported format - do nothing
                }
            }
        }
    }

    /// Fill a rectangle with a solid color
    ///
    /// # Safety
    /// The caller must ensure that the framebuffer memory is valid and accessible.
    pub unsafe fn fill_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        color: Color,
    ) {
        for py in y..core::cmp::min(y + h, self.height) {
            for px in x..core::cmp::min(x + w, self.width) {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// Clear the entire framebuffer with a solid color
    ///
    /// # Safety
    /// The caller must ensure that the framebuffer memory is valid and accessible.
    pub unsafe fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Scroll the framebuffer up by the specified number of lines
    ///
    /// # Arguments
    /// * `lines` - Number of lines to scroll
    /// * `char_height` - Height of a character cell in pixels (for line height)
    ///
    /// # Safety
    /// The caller must ensure that the framebuffer memory is valid and accessible.
    pub unsafe fn scroll(&mut self, lines: usize, char_height: usize) {
        let scroll_pixels = lines * char_height;

        if scroll_pixels >= self.height {
            // Scrolling past screen height - just clear
            self.clear(Color::BLACK);
            return;
        }

        // Move pixels up
        let fb_ptr = self.base_addr as *mut u8;
        let row_size = self.pitch;

        for y in scroll_pixels..self.height {
            let src_offset = y * row_size;
            let dst_offset = (y - scroll_pixels) * row_size;

            for x in 0..row_size {
                *fb_ptr.add(dst_offset + x) = *fb_ptr.add(src_offset + x);
            }
        }

        // Clear the bottom area
        let clear_start = self.height - scroll_pixels;
        self.fill_rect(0, clear_start, self.width, scroll_pixels, Color::BLACK);
    }

    /// Write text to the framebuffer (placeholder - requires font rendering)
    ///
    /// # Arguments
    /// * `text` - Text to write
    /// * `x` - X position in pixels
    /// * `y` - Y position in pixels
    /// * `fg_color` - Foreground color
    /// * `bg_color` - Background color
    ///
    /// # Note
    /// This is a placeholder. The actual text rendering will be implemented
    /// in the console module with font support.
    pub fn write_text(
        &mut self,
        text: &str,
        x: usize,
        y: usize,
        fg_color: Color,
        _bg_color: Color,
    ) {
        // Placeholder: just draw some colored rectangles for now
        let char_width = 8; // Typical monospace character width
        let mut current_x = x;

        for _ch in text.chars() {
            unsafe {
                self.fill_rect(current_x, y, char_width - 1, 16, fg_color);
            }
            current_x += char_width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_new() {
        let c = Color::new(255, 128, 0);
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_color_constants() {
        assert_eq!(Color::BLACK, Color::new(0, 0, 0));
        assert_eq!(Color::WHITE, Color::new(255, 255, 255));
    }

    #[test]
    fn test_color_rgb565_conversion() {
        let c = Color::new(255, 0, 0); // Red
        let rgb565 = c.to_rgb565();
        let c2 = Color::from_rgb565(rgb565);

        // Due to precision loss in 5-bit conversion, we allow small errors
        assert!(c2.r >= 250); // Should be close to 255
        assert_eq!(c2.g, 0);
        assert_eq!(c2.b, 0);
    }

    #[test]
    fn test_framebuffer_new() {
        let fb = Framebuffer::new(
            0xE0000000,
            1024,
            768,
            4096,
            32,
            PixelFormat::RGB,
        );

        assert_eq!(fb.base_addr, 0xE0000000);
        assert_eq!(fb.width, 1024);
        assert_eq!(fb.height, 768);
        assert_eq!(fb.pitch, 4096);
        assert_eq!(fb.bpp, 32);
    }

    #[test]
    fn test_framebuffer_size() {
        let fb = Framebuffer::new(
            0xE0000000,
            1024,
            768,
            4096,
            32,
            PixelFormat::RGB,
        );

        assert_eq!(fb.size(), 768 * 4096);
    }

    #[test]
    fn test_pixel_offset_valid() {
        let fb = Framebuffer::new(0xE0000000, 1024, 768, 4096, 32, PixelFormat::RGB);
        assert_eq!(fb.pixel_offset(100, 200), Some(200 * 4096 + 100 * 4));
    }

    #[test]
    fn test_pixel_offset_invalid() {
        let fb = Framebuffer::new(0xE0000000, 1024, 768, 4096, 32, PixelFormat::RGB);
        assert_eq!(fb.pixel_offset(2000, 200), None); // x out of bounds
        assert_eq!(fb.pixel_offset(100, 1000), None); // y out of bounds
    }
}
