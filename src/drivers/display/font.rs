// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! PSF2 Font Support
//!
//! This module provides support for PSF2 (PC Screen Font 2) fonts,
//! which are commonly used for Linux console text rendering.

/// PSF2 font header
///
/// The PSF2 font format consists of a header followed by glyph data.
/// This structure represents the header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Psf2Header {
    /// Magic bytes (PSF2 magic: 0x72 0xb5 0x4a 0x86)
    pub magic: u32,
    /// Version (zero)
    pub headersize: u32,
    /// Flags (see PSF2_FLAGS_* constants)
    pub flags: u32,
    /// Number of glyphs
    pub length: u32,
    /// Number of bytes per glyph
    pub charsize: u32,
    /// Height in pixels
    pub height: u32,
    /// Width in pixels
    pub width: u32,
}

/// PSF2 font magic value
pub const PSF2_MAGIC: u32 = 0x864AB572;

/// PSF2 font has a unicode table
pub const PSF2_HAS_UNICODE_TABLE: u32 = 0x01;

/// PSF2 Font wrapper
///
/// This struct wraps a PSF2 font and provides methods for rendering glyphs.
pub struct Psf2Font {
    header: Psf2Header,
    glyph_data: &'static [u8],
}

impl Psf2Font {
    /// Create a PSF2 font from raw data
    ///
    /// # Arguments
    /// * `data` - Raw PSF2 font data (header + glyph data)
    ///
    /// # Returns
    /// * `Ok(font)` - Successfully parsed font
    /// * `Err(&str)` - Invalid PSF2 font data
    pub unsafe fn from_data(data: &'static [u8]) -> Result<Self, &'static str> {
        if data.len() < core::mem::size_of::<Psf2Header>() {
            return Err("Font data too small for PSF2 header");
        }

        let header = &*(data.as_ptr() as *const Psf2Header);

        if header.magic != PSF2_MAGIC {
            return Err("Invalid PSF2 magic value");
        }

        let header_size = header.headersize as usize;
        let glyph_data_size = (header.length * header.charsize) as usize;

        if data.len() < header_size + glyph_data_size {
            return Err("Font data too small for declared glyphs");
        }

        let glyph_data = &data[header_size..header_size + glyph_data_size];

        Ok(Self {
            header: *header,
            glyph_data,
        })
    }

    /// Get the font height in pixels
    pub fn height(&self) -> usize {
        self.header.height as usize
    }

    /// Get the font width in pixels
    pub fn width(&self) -> usize {
        self.header.width as usize
    }

    /// Get the number of glyphs in the font
    pub fn glyph_count(&self) -> usize {
        self.header.length as usize
    }

    /// Get the bytes per glyph
    pub fn bytes_per_glyph(&self) -> usize {
        self.header.charsize as usize
    }

    /// Check if a specific pixel in a glyph is set
    ///
    /// # Arguments
    /// * `ch` - Character code (typically ASCII or Unicode index)
    /// * `x` - X position within the glyph (0 to width-1)
    /// * `y` - Y position within the glyph (0 to height-1)
    ///
    /// # Returns
    /// * `true` - Pixel is set (foreground)
    /// * `false` - Pixel is clear (background)
    pub fn glyph_pixel(&self, ch: u8, x: usize, y: usize) -> bool {
        let glyph_index = ch as usize;
        let width = self.width();
        let height = self.height();

        if glyph_index >= self.glyph_count() || x >= width || y >= height {
            return false;
        }

        let bytes_per_row = (width + 7) / 8;
        let glyph_offset = glyph_index * self.bytes_per_glyph();
        let row_offset = y * bytes_per_row;
        let byte_offset = glyph_offset + row_offset + (x / 8);

        if byte_offset >= self.glyph_data.len() {
            return false;
        }

        let bit_mask = 1 << (7 - (x % 8));
        (self.glyph_data[byte_offset] & bit_mask) != 0
    }

    /// Check if the font has a Unicode table
    pub fn has_unicode_table(&self) -> bool {
        (self.header.flags & PSF2_HAS_UNICODE_TABLE) != 0
    }
}

/// Default 8x16 VGA font (partial - just a few characters for testing)
///
/// This is a minimal subset of the standard VGA 8x16 font.
/// In a real implementation, this would be replaced with a complete
/// PSF2 font embedded via build.rs or include_bytes!.
///
/// For now, we'll provide a simple 8x16 bitmap font for ASCII characters.
pub struct SimpleVgaFont;

impl SimpleVgaFont {
    /// Get the height of the font
    pub const fn height() -> usize {
        16
    }

    /// Get the width of the font
    pub const fn width() -> usize {
        8
    }

    /// Check if a pixel is set in a glyph
    pub fn glyph_pixel(ch: u8, x: usize, y: usize) -> bool {
        if x >= 8 || y >= 16 {
            return false;
        }

        // Simple bitmap font data for basic ASCII characters
        // Each row is 8 bits (1 byte), 16 rows per character
        let font_data = Self::get_glyph_data(ch);

        // Check if the bit is set
        let bit_mask = 1 << (7 - x);
        (font_data[y] & bit_mask) != 0
    }

    /// Get the glyph data for a character
    fn get_glyph_data(ch: u8) -> [u8; 16] {
        // Very simple 8x16 bitmap font for a few characters
        match ch {
            b'A' => [
                0b00000000,
                0b00111100,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01111110,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b00000000,
                0b00000000,
                0b00000000,
                0b00000000,
            ],
            b'B' => [
                0b00000000,
                0b01111100,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01111100,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01111100,
                0b00000000,
                0b00000000,
                0b00000000,
                0b00000000,
            ],
            b'C' => [
                0b00000000,
                0b00111110,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b01000000,
                0b00111110,
                0b00000000,
                0b00000000,
                0b00000000,
                0b00000000,
            ],
            // Space character (all zeros)
            b' ' => [0; 16],
            // For other characters, return a simple box pattern
            _ => [
                0b00000000,
                0b01111110,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01000010,
                0b01111110,
                0b00000000,
                0b00000000,
                0b00000000,
                0b00000000,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_psf2_magic() {
        assert_eq!(PSF2_MAGIC, 0x864AB572);
    }

    #[test]
    fn test_simple_vga_font_dimensions() {
        assert_eq!(SimpleVgaFont::height(), 16);
        assert_eq!(SimpleVgaFont::width(), 8);
    }

    #[test]
    fn test_simple_vga_font_a() {
        // Test 'A' character - should have some pixels set
        assert!(SimpleVgaFont::glyph_pixel(b'A', 2, 1)); // Top bar
        assert!(SimpleVgaFont::glyph_pixel(b'A', 1, 7)); // Middle bar
        assert!(SimpleVgaFont::glyph_pixel(b'A', 5, 13)); // Bottom
    }

    #[test]
    fn test_simple_vga_font_space() {
        // Space character should have no pixels set
        for y in 0..16 {
            for x in 0..8 {
                assert!(!SimpleVgaFont::glyph_pixel(b' ', x, y));
            }
        }
    }

    #[test]
    fn test_simple_vga_font_bounds() {
        // Out of bounds pixels should always be false
        assert!(!SimpleVgaFont::glyph_pixel(b'A', 10, 5)); // x out of bounds
        assert!(!SimpleVgaFont::glyph_pixel(b'A', 5, 20)); // y out of bounds
    }
}
