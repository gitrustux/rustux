// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! Scheduling (stub for now)

/// Scheduler stub
pub struct Scheduler {
    pub running: bool,
}

impl Scheduler {
    pub fn new() -> Self {
        Self { running: false }
    }

    pub fn start(&mut self) {
        self.running = true;
    }
}
