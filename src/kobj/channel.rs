// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Channels: a pair of endpoints that carry messages of bytes and handles.
//!
//! Writing to one endpoint queues the message on the other. A handle that is
//! written leaves the sender's handle table and lives in the message until
//! it is read into the receiver's table (or the message is destroyed, which
//! closes it).

use super::{signals, KObject, Rights, SignalState};
use crate::arch::amd64::mm::RxStatus;
use crate::sync::SpinMutex;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

/// Largest message payload in bytes (Zircon: 64 KiB)
pub const MAX_MSG_BYTES: usize = 65536;
/// Most handles in one message (Zircon: 64)
pub const MAX_MSG_HANDLES: usize = 64;
/// Most unread messages queued on one endpoint
pub const MAX_PENDING: usize = 1024;

pub struct Message {
    pub data: Vec<u8>,
    pub handles: Vec<(KObject, Rights)>,
}

pub struct Channel {
    pub signals: SignalState,
    peer: SpinMutex<Weak<Channel>>,
    /// Messages waiting to be read from this endpoint
    queue: SpinMutex<VecDeque<Message>>,
}

impl Channel {
    pub fn new_pair() -> (Arc<Self>, Arc<Self>) {
        let mk = || Arc::new(Self {
            signals: SignalState::new(signals::WRITABLE),
            peer: SpinMutex::new(Weak::new()),
            queue: SpinMutex::new(VecDeque::new()),
        });
        let (a, b) = (mk(), mk());
        *a.peer.lock() = Arc::downgrade(&b);
        *b.peer.lock() = Arc::downgrade(&a);
        (a, b)
    }

    pub fn peer(&self) -> Option<Arc<Channel>> {
        self.peer.lock().upgrade()
    }

    /// Queue a message on the other endpoint. On failure the message is
    /// handed back so its handles are closed by the caller (outside locks).
    pub fn write(&self, msg: Message) -> Result<(), (RxStatus, Message)> {
        let Some(peer) = self.peer() else { return Err((RxStatus::ERR_PEER_CLOSED, msg)) };
        {
            let mut q = peer.queue.lock();
            if q.len() >= MAX_PENDING {
                return Err((RxStatus::ERR_SHOULD_WAIT, msg));
            }
            q.push_back(msg);
        }
        peer.signals.update(0, signals::READABLE);
        Ok(())
    }

    /// Size (bytes, handles) of the next message, if any
    pub fn peek_size(&self) -> Option<(usize, usize)> {
        self.queue.lock().front().map(|m| (m.data.len(), m.handles.len()))
    }

    /// Take the next message
    pub fn take(&self) -> Option<Message> {
        let (msg, empty) = {
            let mut q = self.queue.lock();
            let m = q.pop_front();
            (m, q.is_empty())
        };
        if empty {
            self.signals.update(signals::READABLE, 0);
        }
        msg
    }

    /// Put a message back at the front (the reader could not take it)
    pub fn untake(&self, msg: Message) {
        self.queue.lock().push_front(msg);
        self.signals.update(0, signals::READABLE);
    }

    /// Whether `other` is this endpoint or its peer
    pub fn is_self_or_peer(&self, other: &Arc<Channel>) -> bool {
        core::ptr::eq(self, Arc::as_ptr(other))
            || self.peer.lock().as_ptr() == Arc::as_ptr(other)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.lock().upgrade() {
            peer.signals.update(signals::WRITABLE, signals::PEER_CLOSED);
        }
        // Unread messages (and the handles in them) are dropped with the queue
    }
}
