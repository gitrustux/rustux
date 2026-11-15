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
use core::sync::atomic::{AtomicU32, Ordering};

/// Largest message payload in bytes (Zircon: 64 KiB)
pub const MAX_MSG_BYTES: usize = 65536;
/// Most handles in one message (Zircon: 64)
pub const MAX_MSG_HANDLES: usize = 64;
/// Most unread messages queued on one endpoint
pub const MAX_PENDING: usize = 1024;

pub struct Message {
    pub data: Vec<u8>,
    pub handles: Vec<(KObject, Rights)>,
    /// Who wrote it (step 17): a copy of the writer's credentials
    pub sender: Sender,
}

/// The sender of a message, as `channel_read` with `CHANNEL_READ_CREDS`
/// returns it after the two counts
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Sender {
    /// The writing process's koid (0: the kernel)
    pub pid: u64,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub ngroups: u32,
    pub _pad: u32,
    pub groups: [u32; crate::kobj::task::MAX_GROUPS],
}

impl Sender {
    /// Messages the kernel itself writes
    pub const KERNEL: Sender = Sender { pid: 0, uid: 0, gid: 0, euid: 0, egid: 0, ngroups: 0, _pad: 0, groups: [0; crate::kobj::task::MAX_GROUPS] };

    pub fn of(koid: u64, c: &crate::kobj::task::Creds) -> Sender {
        Sender { pid: koid, uid: c.ruid, gid: c.rgid, euid: c.euid, egid: c.egid, ngroups: c.ngroups, _pad: 0, groups: c.groups }
    }
}

/// `channel_read` option: also return the sender
pub const READ_CREDS: usize = 1;

pub struct Channel {
    pub signals: SignalState,
    peer: SpinMutex<Weak<Channel>>,
    /// Messages waiting to be read from this endpoint
    queue: SpinMutex<VecDeque<Message>>,
    /// A terminal's size (columns << 16 | rows; 0 unknown): a channel
    /// that is a console keeps it for both ends (property TTY_SIZE)
    tty_size: AtomicU32,
}

impl Channel {
    pub fn new_pair() -> (Arc<Self>, Arc<Self>) {
        let mk = || Arc::new(Self {
            signals: SignalState::new(signals::WRITABLE),
            peer: SpinMutex::new(Weak::new()),
            queue: SpinMutex::new(VecDeque::new()),
            tty_size: AtomicU32::new(0),
        });
        let (a, b) = (mk(), mk());
        *a.peer.lock() = Arc::downgrade(&b);
        *b.peer.lock() = Arc::downgrade(&a);
        (a, b)
    }

    pub fn peer(&self) -> Option<Arc<Channel>> {
        self.peer.lock().upgrade()
    }

    /// The terminal size (columns << 16 | rows), 0 if never set
    pub fn tty_size(&self) -> u32 {
        self.tty_size.load(Ordering::Relaxed)
    }

    /// Set the terminal size on both ends
    pub fn set_tty_size(&self, v: u32) {
        self.tty_size.store(v, Ordering::Relaxed);
        if let Some(p) = self.peer() {
            p.tty_size.store(v, Ordering::Relaxed);
        }
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
