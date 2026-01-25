// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Scancode to ASCII translation
//!
//! This module provides translation tables for converting PS/2 keyboard
//! scancodes (set 1) to ASCII characters.

/// Special key codes (non-ASCII keys)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpecialKey {
    // Modifier keys
    LeftShift = 0x80,
    RightShift,
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    CapsLock,

    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    // Special keys
    Escape,
    Tab,
    Enter,
    Backspace,

    // Arrow keys
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,

    // Navigation keys
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,

    // Other
    PrintScreen,
    ScrollLock,
    Pause,
}

/// Key event type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// Regular ASCII character
    Ascii(u8),

    /// Special key (modifier, function, arrow, etc.)
    Special(SpecialKey),

    /// Key release (only tracked for modifiers)
    Release(u8),
}

/// US QWERTY scancode set 1 to ASCII translation (lowercase)
pub const SCANCODE_TO_ASCII_LOWER: &[u8; 128] = &[
    0x00, // 0x00: Unknown
    0x00, // 0x01: Esc (handled separately)
    b'1', // 0x02
    b'2', // 0x03
    b'3', // 0x04
    b'4', // 0x05
    b'5', // 0x06
    b'6', // 0x07
    b'7', // 0x08
    b'8', // 0x09
    b'9', // 0x0A
    b'0', // 0x0B
    b'-', // 0x0C
    b'=', // 0x0D
    0x08, // 0x0E: Backspace
    0x09, // 0x0F: Tab
    b'q', // 0x10
    b'w', // 0x11
    b'e', // 0x12
    b'r', // 0x13
    b't', // 0x14
    b'y', // 0x15
    b'u', // 0x16
    b'i', // 0x17
    b'o', // 0x18
    b'p', // 0x19
    b'[', // 0x1A
    b']', // 0x1B
    0x0A, // 0x1C: Enter
    0x00, // 0x1D: Left Ctrl (modifier)
    b'a', // 0x1E
    b's', // 0x1F
    b'd', // 0x20
    b'f', // 0x21
    b'g', // 0x22
    b'h', // 0x23
    b'j', // 0x24
    b'k', // 0x25
    b'l', // 0x26
    b';', // 0x27
    b'\'', // 0x28
    b'`', // 0x29
    0x00, // 0x2A: Left Shift (modifier)
    b'\\', // 0x2B
    b'z', // 0x2C
    b'x', // 0x2D
    b'c', // 0x2E
    b'v', // 0x2F
    b'b', // 0x30
    b'n', // 0x31
    b'm', // 0x32
    b',', // 0x33
    b'.', // 0x34
    b'/', // 0x35
    0x00, // 0x36: Right Shift (modifier)
    b'*', // 0x37: Print Screen * (keypad)
    0x00, // 0x38: Left Alt (modifier)
    b' ', // 0x39: Space
    0x00, // 0x3A: Caps Lock
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x3B-0x44: F1-F10
    0x00, 0x00, 0x00, // 0x45-0x47: F11-F12, etc.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x48-0x51
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x52-0x5B
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x5C-0x65
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x66-0x6F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x70-0x79
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x7A-0x7F
];

/// US QWERTY scancode set 1 to ASCII translation (uppercase/shifted)
pub const SCANCODE_TO_ASCII_UPPER: &[u8; 128] = &[
    0x00, // 0x00: Unknown
    0x00, // 0x01: Esc (handled separately)
    b'!', // 0x02
    b'@', // 0x03
    b'#', // 0x04
    b'$', // 0x05
    b'%', // 0x06
    b'^', // 0x07
    b'&', // 0x08
    b'*', // 0x09
    b'(', // 0x0A
    b')', // 0x0B
    b'_', // 0x0C
    b'+', // 0x0D
    0x08, // 0x0E: Backspace
    0x09, // 0x0F: Tab
    b'Q', // 0x10
    b'W', // 0x11
    b'E', // 0x12
    b'R', // 0x13
    b'T', // 0x14
    b'Y', // 0x15
    b'U', // 0x16
    b'I', // 0x17
    b'O', // 0x18
    b'P', // 0x19
    b'{', // 0x1A
    b'}', // 0x1B
    0x0A, // 0x1C: Enter
    0x00, // 0x1D: Left Ctrl (modifier)
    b'A', // 0x1E
    b'S', // 0x1F
    b'D', // 0x20
    b'F', // 0x21
    b'G', // 0x22
    b'H', // 0x23
    b'J', // 0x24
    b'K', // 0x25
    b'L', // 0x26
    b':', // 0x27
    b'"', // 0x28
    b'~', // 0x29
    0x00, // 0x2A: Left Shift (modifier)
    b'|', // 0x2B
    b'Z', // 0x2C
    b'X', // 0x2D
    b'C', // 0x2E
    b'V', // 0x2F
    b'B', // 0x30
    b'N', // 0x31
    b'M', // 0x32
    b'<', // 0x33
    b'>', // 0x34
    b'?', // 0x35
    0x00, // 0x36: Right Shift (modifier)
    b'*', // 0x37: Print Screen * (keypad)
    0x00, // 0x38: Left Alt (modifier)
    b' ', // 0x39: Space
    0x00, // 0x3A: Caps Lock
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // F1-F10
    0x00, 0x00, 0x00, // F11-F12, etc.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Extended scancode table (prefixed with 0xE0)
/// These are for special keys like arrow keys, home/end, etc.
pub const SCANCODE_TO_ASCII_E0: &[u8; 128] = &[
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x00-0x0F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x10-0x1F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x20-0x2F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x30-0x3F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x40-0x4F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x50-0x5F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x60-0x6F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 0x70-0x7F
];

/// Modifier key state
#[derive(Debug, Clone, Copy)]
pub struct ModifierState {
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub caps_lock: bool,
}

impl ModifierState {
    pub const fn new() -> Self {
        Self {
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            caps_lock: false,
        }
    }

    pub fn shift(&self) -> bool {
        self.left_shift || self.right_shift
    }

    pub fn ctrl(&self) -> bool {
        self.left_ctrl || self.right_ctrl
    }

    pub fn alt(&self) -> bool {
        self.left_alt || self.right_alt
    }
}

/// Convert a scancode to a KeyEvent
///
/// # Arguments
/// * `scancode` - The raw scancode from the keyboard
/// * `modifiers` - Current modifier state
/// * `extended` - True if this is an extended scancode (prefixed with 0xE0)
///
/// # Returns
/// * `KeyEvent::Ascii(c)` - Regular ASCII character
/// * `KeyEvent::Special(key)` - Special key
/// * `KeyEvent::Release(code)` - Key release (for modifier tracking)
pub fn scancode_to_keyevent(scancode: u8, modifiers: &ModifierState, extended: bool) -> KeyEvent {
    let is_release = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    // Handle modifier keys
    match code {
        0x2A => return if is_release {
            KeyEvent::Release(0x2A) // Left Shift release
        } else {
            KeyEvent::Special(SpecialKey::LeftShift)
        },
        0x36 => return if is_release {
            KeyEvent::Release(0x36) // Right Shift release
        } else {
            KeyEvent::Special(SpecialKey::RightShift)
        },
        0x1D => return if is_release {
            KeyEvent::Release(if extended { 0x1D | 0x80 } else { 0x1D }) // Ctrl release
        } else {
            KeyEvent::Special(if extended { SpecialKey::RightCtrl } else { SpecialKey::LeftCtrl })
        },
        0x38 => return if is_release {
            KeyEvent::Release(if extended { 0x38 | 0x80 } else { 0x38 }) // Alt release
        } else {
            KeyEvent::Special(if extended { SpecialKey::RightAlt } else { SpecialKey::LeftAlt })
        },
        0x3A => return KeyEvent::Special(SpecialKey::CapsLock),
        0x01 => return KeyEvent::Special(SpecialKey::Escape),
        0x0E => return KeyEvent::Special(SpecialKey::Backspace),
        0x0F => return KeyEvent::Special(SpecialKey::Tab),
        0x1C => return KeyEvent::Special(SpecialKey::Enter),
        _ => {}
    }

    // Handle extended keys
    if extended {
        return match code {
            0x48 => KeyEvent::Special(SpecialKey::ArrowUp),
            0x50 => KeyEvent::Special(SpecialKey::ArrowDown),
            0x4B => KeyEvent::Special(SpecialKey::ArrowLeft),
            0x4D => KeyEvent::Special(SpecialKey::ArrowRight),
            0x47 => KeyEvent::Special(SpecialKey::Home),
            0x4F => KeyEvent::Special(SpecialKey::End),
            0x49 => KeyEvent::Special(SpecialKey::PageUp),
            0x51 => KeyEvent::Special(SpecialKey::PageDown),
            0x52 => KeyEvent::Special(SpecialKey::Insert),
            0x53 => KeyEvent::Special(SpecialKey::Delete),
            _ => KeyEvent::Special(SpecialKey::Escape), // Unknown extended key
        };
    }

    // Skip release codes for regular keys
    if is_release {
        return KeyEvent::Release(code);
    }

    // Regular ASCII keys - use appropriate table based on shift state
    let shift = modifiers.shift() ^ modifiers.caps_lock;
    let table = if shift {
        SCANCODE_TO_ASCII_UPPER
    } else {
        SCANCODE_TO_ASCII_LOWER
    };

    if (code as usize) < table.len() {
        let ascii = table[code as usize];
        if ascii != 0 {
            return KeyEvent::Ascii(ascii);
        }
    }

    // Unknown scancode - return as special key
    KeyEvent::Special(SpecialKey::Escape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_state_new() {
        let m = ModifierState::new();
        assert!(!m.shift());
        assert!(!m.ctrl());
        assert!(!m.alt());
    }

    #[test]
    fn test_modifier_state_shift() {
        let mut m = ModifierState::new();
        assert!(!m.shift());
        m.left_shift = true;
        assert!(m.shift());
        m.left_shift = false;
        m.right_shift = true;
        assert!(m.shift());
    }

    #[test]
    fn test_scancode_to_ascii_basic() {
        let m = ModifierState::new();
        match scancode_to_keyevent(0x1E, &m, false) { // 'a' key
            KeyEvent::Ascii(b'a') => {}
            _ => panic!("Expected 'a'"),
        }
    }

    #[test]
    fn test_scancode_to_ascii_shifted() {
        let mut m = ModifierState::new();
        m.left_shift = true;
        match scancode_to_keyevent(0x1E, &m, false) { // 'a' key with shift
            KeyEvent::Ascii(b'A') => {}
            _ => panic!("Expected 'A'"),
        }
    }
}
