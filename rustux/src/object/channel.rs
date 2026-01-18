// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! IPC Channels
//!
//! Channels provide bidirectional message passing between processes.
//! They support sending both bytes and handles (capability transfer).
//!
//! # Design
//!
//! - **Bidirectional**: Created as pairs of endpoints
//! - **FIFO ordering**: Messages delivered in order
//! - **Bounded queue**: Backpressure when full
//! - **Handle passing**: Handles can be transferred with rights reduction
//! - **Peer closure**: One end closed → PEER_CLOSED signal to other
//!
//! # Usage
//!
//! ```rust
//! let (channel_a, channel_b) = Channel::create()?;
//! channel_a.write(&data, &handles)?;
//! let (msg, handles) = channel_b.read(&mut buf)?;
//! ```

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::SpinMutex;
use crate::object::handle::{KernelObjectBase, ObjectType, Handle};
use crate::object::event::Event;
use alloc::vec::Vec;
use alloc::collections::VecDeque;

/// ============================================================================
/// Channel ID
/// ============================================================================

/// Channel identifier
pub type ChannelId = u64;

/// Next channel ID counter
static mut NEXT_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new channel ID
fn alloc_channel_id() -> ChannelId {
    unsafe { NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed) }
}

/// ============================================================================
/// Message
/// ============================================================================

/// Maximum message size in bytes
pub const MAX_MSG_SIZE: usize = 64 * 1024;

/// Maximum handles per message
pub const MAX_MSG_HANDLES: usize = 64;

/// Message data
pub struct Message {
    /// Message bytes
    pub data: Vec<u8>,

    /// Handles being transferred
    pub handles: Vec<Handle>,
}

impl Message {
    /// Create a new message
    pub fn new(data: Vec<u8>, handles: Vec<Handle>) -> Self {
        Self { data, handles }
    }

    /// Get message data size
    pub fn data_size(&self) -> usize {
        self.data.len()
    }

    /// Get handle count
    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }

    /// Check if message is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() && self.handles.is_empty()
    }
}

/// ============================================================================
/// Channel State
/// ============================================================================

/// Channel state
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    /// Channel is active
    Active = 0,

    /// One endpoint closed
    PeerClosed = 1,

    /// Both endpoints closed
    Closed = 2,
}

impl ChannelState {
    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::PeerClosed,
            2 => Self::Closed,
            _ => Self::Active,
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self as u32
    }
}

/// ============================================================================
/// Channel
/// ============================================================================

/// Read result
#[derive(Debug)]
pub struct ReadResult {
    /// Number of bytes read
    pub bytes_read: usize,

    /// Number of handles read
    pub handles_read: usize,
}

/// Channel endpoint
///
/// Represents one endpoint of a bidirectional channel.
pub struct Channel {
    /// Kernel object base
    pub base: KernelObjectBase,

    /// Channel ID
    pub id: ChannelId,

    /// Peer channel ID
    pub peer: SpinMutex<Option<ChannelId>>,

    /// Message queue
    pub queue: SpinMutex<VecDeque<Message>>,

    /// Maximum queue depth (in bytes)
    pub max_queue_bytes: usize,

    /// Current queue size (in bytes)
    pub queue_size: AtomicUsize,

    /// Read event (signaled when messages available)
    pub read_event: SpinMutex<Event>,

    /// Write event (signaled when space available)
    pub write_event: SpinMutex<Event>,

    /// Channel state
    pub state: SpinMutex<ChannelState>,
}

impl Channel {
    /// Create a new channel
    fn new(id: ChannelId, max_queue_bytes: usize) -> Self {
        Self {
            base: KernelObjectBase::new(ObjectType::Channel),
            id,
            peer: SpinMutex::new(None),
            queue: SpinMutex::new(VecDeque::new()),
            max_queue_bytes,
            queue_size: AtomicUsize::new(0),
            read_event: SpinMutex::new(Event::new(
                false,
                crate::object::event::EventFlags::empty,
            )),
            write_event: SpinMutex::new(Event::new(
                true, // Initially writable (empty queue)
                crate::object::event::EventFlags::empty,
            )),
            state: SpinMutex::new(ChannelState::Active),
        }
    }

    /// Create a channel pair
    ///
    /// # Returns
    ///
    /// Tuple of (channel_a, channel_b)
    pub fn create() -> Result<(Self, Self), &'static str> {
        let id_a = alloc_channel_id();
        let id_b = alloc_channel_id();

        let max_queue_bytes = 256 * 1024; // 256KB default

        let channel_a = Self::new(id_a, max_queue_bytes);
        let channel_b = Self::new(id_b, max_queue_bytes);

        // Link peers
        *channel_a.peer.lock() = Some(id_b);
        *channel_b.peer.lock() = Some(id_a);

        Ok((channel_a, channel_b))
    }

    /// Get channel ID
    pub const fn id(&self) -> ChannelId {
        self.id
    }

    /// Get peer channel ID
    pub fn peer_id(&self) -> Option<ChannelId> {
        *self.peer.lock()
    }

    /// Get channel state
    pub fn state(&self) -> ChannelState {
        *self.state.lock()
    }

    /// Write data and handles to the channel
    ///
    /// # Arguments
    ///
    /// * `data` - Data bytes to write
    /// * `handles` - Handles to transfer
    pub fn write(&self, data: &[u8], handles: &[Handle]) -> Result<usize, &'static str> {
        let state = *self.state.lock();
        if state != ChannelState::Active {
            return Err("channel not active");
        }

        // Check message size limits
        if data.len() > MAX_MSG_SIZE {
            return Err("message too large");
        }

        if handles.len() > MAX_MSG_HANDLES {
            return Err("too many handles");
        }

        // Check queue space
        let msg_size = data.len();
        let current_size = self.queue_size.load(Ordering::Acquire);

        if current_size + msg_size > self.max_queue_bytes {
            return Err("channel full");
        }

        // Copy data and handles
        let msg_data = Vec::from(data);
        let msg_handles = handles.to_vec();

        // Add to queue
        {
            let mut queue = self.queue.lock();
            queue.push_back(Message::new(msg_data, msg_handles));
        }

        // Update queue size
        self.queue_size.fetch_add(msg_size, Ordering::Release);

        // Signal read event
        self.read_event.lock().signal();

        Ok(data.len())
    }

    /// Read data and handles from the channel
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer to read data into
    /// * `handle_buf` - Buffer to read handles into
    ///
    /// # Returns
    ///
    /// Read result with bytes/handles read counts
    pub fn read(
        &self,
        buf: &mut [u8],
        handle_buf: &mut [Handle],
    ) -> Result<ReadResult, &'static str> {
        // Try to get a message from queue
        let (data, handles) = {
            let mut queue = self.queue.lock();
            match queue.pop_front() {
                Some(msg) => (msg.data, msg.handles),
                None => {
                    // Check if peer closed
                    if *self.state.lock() == ChannelState::PeerClosed {
                        // Return peer closed status
                        return Err("peer closed");
                    }
                    return Err("no messages");
                }
            }
        };

        // Update queue size
        let msg_size = data.len();
        self.queue_size.fetch_sub(msg_size, Ordering::Release);

        // Copy data to buffer
        let bytes_to_copy = core::cmp::min(buf.len(), data.len());
        buf[..bytes_to_copy].copy_from_slice(&data[..bytes_to_copy]);

        // Copy handles to buffer
        let handles_to_copy = core::cmp::min(handle_buf.len(), handles.len());
        for (i, handle) in handles.iter().take(handles_to_copy).enumerate() {
            handle_buf[i] = handle.clone();
        }

        // Signal write event (space available)
        self.write_event.lock().signal();

        Ok(ReadResult {
            bytes_read: bytes_to_copy,
            handles_read: handles_to_copy,
        })
    }

    /// Get the number of messages in the queue
    pub fn queue_len(&self) -> usize {
        self.queue.lock().len()
    }

    /// Get the current queue size in bytes
    pub fn queue_size(&self) -> usize {
        self.queue_size.load(Ordering::Acquire)
    }

    /// Close the channel endpoint
    ///
    /// Returns true if this was the last close.
    pub fn close(&self) -> bool {
        *self.state.lock() = ChannelState::Closed;

        // Signal peer (if exists) that we closed
        // TODO: Implement peer notification

        // Signal read event (to wake readers)
        self.read_event.lock().signal();

        // Decrement ref count
        self.base.ref_dec()
    }

    /// Get the kernel object base
    pub fn base(&self) -> &KernelObjectBase {
        &self.base
    }

    /// Get reference count
    pub fn ref_count(&self) -> usize {
        self.base.ref_count()
    }

    /// Increment reference count
    pub fn ref_inc(&self) {
        self.base.ref_inc();
    }

    /// Decrement reference count
    ///
    /// Returns true if this was the last reference.
    pub fn ref_dec(&self) -> bool {
        self.base.ref_dec()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_create() {
        let (ch_a, ch_b) = Channel::create().unwrap();

        assert_eq!(ch_a.state(), ChannelState::Active);
        assert_eq!(ch_b.state(), ChannelState::Active);

        assert_eq!(ch_a.peer_id(), Some(ch_b.id()));
        assert_eq!(ch_b.peer_id(), Some(ch_a.id()));
    }

    #[test]
    fn test_message() {
        let data = vec![1, 2, 3, 4];
        let handles = vec![];

        let msg = Message::new(data.clone(), handles);

        assert_eq!(msg.data_size(), 4);
        assert_eq!(msg.handle_count(), 0);
        assert!(!msg.is_empty());
    }

    #[test]
    fn test_channel_write_read() {
        let (ch_a, ch_b) = Channel::create().unwrap();

        let data = vec![1, 2, 3, 4];
        ch_a.write(&data, &[]).unwrap();

        assert_eq!(ch_b.queue_len(), 1);

        let mut buf = [0u8; 10];
        let mut handle_buf = [];

        let result = ch_b.read(&mut buf, &mut handle_buf).unwrap();

        assert_eq!(result.bytes_read, 4);
        assert_eq!(result.handles_read, 0);
        assert_eq!(&buf[..4], &data[..]);
    }

    #[test]
    fn test_channel_queue_full() {
        // Create a small channel for testing
        let (ch_a, _) = Channel::create().unwrap();

        // Fill the queue (this would require many large messages)
        // For now, just test that size tracking works
        assert_eq!(ch_a.queue_size(), 0);
    }
}
