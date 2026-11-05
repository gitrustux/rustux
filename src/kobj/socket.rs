// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Sockets (Zircon's stream sockets): a pair of endpoints carrying a byte
//! stream each way, with a bounded buffer so a fast writer waits for a slow
//! reader. Used for pipes.
//!
//! Signals: READABLE (bytes to read), WRITABLE (room in the peer's buffer),
//! PEER_CLOSED. Each endpoint's buffer holds the bytes waiting to be read
//! from it; its lock also orders the signal changes that depend on it, so a
//! writer filling the buffer and a reader draining it can't leave WRITABLE
//! in the wrong state.

use super::{signals, SignalState};
use crate::arch::amd64::mm::RxStatus;
use crate::sync::SpinMutex;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};

/// Bytes that can wait in one direction
pub const CAPACITY: usize = 64 * 1024;

pub struct Socket {
    pub signals: SignalState,
    peer: SpinMutex<Weak<Socket>>,
    /// Bytes waiting to be read from this endpoint
    buf: SpinMutex<VecDeque<u8>>,
}

impl Socket {
    pub fn new_pair() -> (Arc<Self>, Arc<Self>) {
        let mk = || Arc::new(Self {
            signals: SignalState::new(signals::WRITABLE),
            peer: SpinMutex::new(Weak::new()),
            buf: SpinMutex::new(VecDeque::new()),
        });
        let (a, b) = (mk(), mk());
        *a.peer.lock() = Arc::downgrade(&b);
        *b.peer.lock() = Arc::downgrade(&a);
        (a, b)
    }

    pub fn peer(&self) -> Option<Arc<Socket>> {
        self.peer.lock().upgrade()
    }

    /// Queue as much of `data` as fits on the other endpoint; returns the
    /// count (at least 1), SHOULD_WAIT if full, PEER_CLOSED if it is gone
    pub fn write(&self, data: &[u8]) -> Result<usize, RxStatus> {
        let peer = self.peer().ok_or(RxStatus::ERR_PEER_CLOSED)?;
        let mut b = peer.buf.lock();
        let room = CAPACITY - b.len();
        if room == 0 {
            self.signals.update(signals::WRITABLE, 0);
            return Err(RxStatus::ERR_SHOULD_WAIT);
        }
        let n = room.min(data.len());
        b.extend(&data[..n]);
        if n > 0 {
            peer.signals.update(0, signals::READABLE);
        }
        if b.len() == CAPACITY {
            self.signals.update(signals::WRITABLE, 0);
        }
        Ok(n)
    }

    /// Take up to `max` bytes; SHOULD_WAIT if none yet, PEER_CLOSED if none
    /// will come
    pub fn read(&self, max: usize) -> Result<alloc::vec::Vec<u8>, RxStatus> {
        let mut b = self.buf.lock();
        if b.is_empty() {
            return Err(if self.peer().is_some() { RxStatus::ERR_SHOULD_WAIT } else { RxStatus::ERR_PEER_CLOSED });
        }
        let n = max.min(b.len());
        let out: alloc::vec::Vec<u8> = b.drain(..n).collect();
        if b.is_empty() {
            self.signals.update(signals::READABLE, 0);
        }
        if let Some(p) = self.peer() {
            p.signals.update(0, signals::WRITABLE);
        }
        Ok(out)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.lock().upgrade() {
            peer.signals.update(signals::WRITABLE, signals::PEER_CLOSED);
        }
    }
}
