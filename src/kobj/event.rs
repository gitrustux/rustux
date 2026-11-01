// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Events and event pairs: objects that exist to be signalled.

use super::{signals, SignalState};
use crate::sync::SpinMutex;
use alloc::sync::{Arc, Weak};

pub struct Event {
    pub signals: SignalState,
}

impl Event {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0) })
    }
}

/// One end of an event pair. Each end can signal the other; when one end
/// is destroyed the other gets `PEER_CLOSED`.
pub struct EventPair {
    pub signals: SignalState,
    peer: SpinMutex<Weak<EventPair>>,
}

impl EventPair {
    pub fn new_pair() -> (Arc<Self>, Arc<Self>) {
        let a = Arc::new(Self { signals: SignalState::new(0), peer: SpinMutex::new(Weak::new()) });
        let b = Arc::new(Self { signals: SignalState::new(0), peer: SpinMutex::new(Arc::downgrade(&a)) });
        *a.peer.lock() = Arc::downgrade(&b);
        (a, b)
    }

    /// The other end, if it still exists
    pub fn peer(&self) -> Option<Arc<EventPair>> {
        self.peer.lock().upgrade()
    }
}

impl Drop for EventPair {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.lock().upgrade() {
            peer.signals.update(0, signals::PEER_CLOSED);
        }
    }
}
