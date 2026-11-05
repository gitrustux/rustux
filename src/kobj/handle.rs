// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Handles and rights.
//!
//! A handle value names a slot in the owning process's table:
//! `(generation << 16) | (index + 1)`. Never 0; a value from a closed slot
//! that has been reused does not match (the generation changed).

use super::KObject;
use alloc::vec::Vec;

/// Rights (values match Zircon's ZX_RIGHT_*)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const DUPLICATE: u32 = 1 << 0;
    pub const TRANSFER: u32 = 1 << 1;
    pub const READ: u32 = 1 << 2;
    pub const WRITE: u32 = 1 << 3;
    pub const EXECUTE: u32 = 1 << 4;
    pub const MAP: u32 = 1 << 5;
    pub const GET_PROPERTY: u32 = 1 << 6;
    pub const SET_PROPERTY: u32 = 1 << 7;
    pub const ENUMERATE: u32 = 1 << 8;
    pub const DESTROY: u32 = 1 << 9;
    pub const SIGNAL: u32 = 1 << 12;
    pub const SIGNAL_PEER: u32 = 1 << 13;
    pub const WAIT: u32 = 1 << 14;
    pub const INSPECT: u32 = 1 << 15;
    pub const MANAGE_JOB: u32 = 1 << 16;
    pub const MANAGE_PROCESS: u32 = 1 << 17;
    pub const MANAGE_THREAD: u32 = 1 << 18;
    pub const SAME_RIGHTS: u32 = 1 << 31;

    pub const BASIC: u32 = Self::DUPLICATE | Self::TRANSFER | Self::WAIT | Self::INSPECT;
    pub const EVENT: u32 = Self::BASIC | Self::SIGNAL;
    pub const EVENTPAIR: u32 = Self::BASIC | Self::SIGNAL | Self::SIGNAL_PEER;
    pub const TIMER: u32 = Self::BASIC | Self::WRITE | Self::SIGNAL;
    pub const VMO: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::MAP | Self::GET_PROPERTY | Self::SET_PROPERTY;
    pub const VMAR: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::EXECUTE;
    pub const PROPERTY: u32 = Self::GET_PROPERTY | Self::SET_PROPERTY;
    pub const PROCESS: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::PROPERTY | Self::ENUMERATE
        | Self::DESTROY | Self::SIGNAL | Self::MANAGE_PROCESS | Self::MANAGE_THREAD;
    pub const THREAD: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::PROPERTY | Self::DESTROY
        | Self::SIGNAL | Self::MANAGE_THREAD;
    pub const JOB: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::PROPERTY | Self::ENUMERATE
        | Self::DESTROY | Self::SIGNAL | Self::MANAGE_JOB | Self::MANAGE_PROCESS | Self::MANAGE_THREAD;
    pub const RESOURCE: u32 = Self::DUPLICATE | Self::TRANSFER | Self::WRITE | Self::INSPECT;
    pub const LOG: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::SIGNAL;
    pub const INTERRUPT: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::SIGNAL;
    pub const SOCKET: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::SIGNAL | Self::SIGNAL_PEER;
    pub const CHANNEL: u32 = Self::BASIC | Self::READ | Self::WRITE | Self::SIGNAL | Self::SIGNAL_PEER;

    pub fn has(self, r: u32) -> bool {
        self.0 & r == r
    }
}

struct Entry {
    object: KObject,
    rights: Rights,
}

struct Slot {
    generation: u16,
    entry: Option<Entry>,
}

/// Maximum handles per process
pub const MAX_HANDLES: usize = 1024;

/// A process's handles
pub struct HandleTable {
    slots: Vec<Slot>,
}

impl HandleTable {
    pub const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    fn index_of(&self, value: u32) -> Option<usize> {
        let index = (value & 0xFFFF) as usize;
        let generation = (value >> 16) as u16;
        if index == 0 || index > self.slots.len() {
            return None;
        }
        let slot = &self.slots[index - 1];
        if slot.generation != generation || slot.entry.is_none() {
            return None;
        }
        Some(index - 1)
    }

    /// Add a handle; returns its value, or None if the table is full
    pub fn add(&mut self, object: KObject, rights: Rights) -> Option<u32> {
        let i = match self.slots.iter().position(|s| s.entry.is_none()) {
            Some(i) => i,
            None => {
                if self.slots.len() >= MAX_HANDLES {
                    return None;
                }
                self.slots.push(Slot { generation: 0, entry: None });
                self.slots.len() - 1
            }
        };
        let slot = &mut self.slots[i];
        slot.entry = Some(Entry { object, rights });
        Some((slot.generation as u32) << 16 | (i as u32 + 1))
    }

    /// Object and rights behind `value`
    pub fn get(&self, value: u32) -> Option<(KObject, Rights)> {
        let i = self.index_of(value)?;
        let e = self.slots[i].entry.as_ref()?;
        Some((e.object.clone(), e.rights))
    }

    /// Remove a handle and return its object (dropped by the caller, outside
    /// any process table lock)
    pub fn remove(&mut self, value: u32) -> Option<(KObject, Rights)> {
        let i = self.index_of(value)?;
        let slot = &mut self.slots[i];
        let e = slot.entry.take()?;
        slot.generation = slot.generation.wrapping_add(1) & 0x7FFF;
        Some((e.object, e.rights))
    }

    /// Remove every handle (process exit); returns the objects to drop
    pub fn take_all(&mut self) -> Vec<KObject> {
        let mut out = Vec::new();
        for slot in self.slots.iter_mut() {
            if let Some(e) = slot.entry.take() {
                out.push(e.object);
                slot.generation = slot.generation.wrapping_add(1) & 0x7FFF;
            }
        }
        out
    }

    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.entry.is_some()).count()
    }
}
