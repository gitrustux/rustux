// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Jobs, processes and threads (Zircon model).
//!
//! - A **process** owns a handle table and an address space. It starts with
//!   no threads; `process_start` runs its first one. It ends when its last
//!   thread exits, when it calls `process_exit`, or when it (or its job) is
//!   killed; then its handles are closed, its mappings dropped, and it gets
//!   the `TERMINATED` signal.
//! - A **thread** is what the scheduler runs: an entry in the process table
//!   (`process::table`, still called `Process` there), linked to its
//!   `Thread` object.
//! - A **job** groups processes and child jobs; killing a job kills them all.
//!
//! Lock order: process table, then a process's `inner`, then its handles.

use super::{signals, Aspace, HandleTable, KObject, SignalState};
use crate::arch::amd64::mm::RxStatus;
use crate::sync::SpinMutex;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Return code of a task killed with `task_kill` (Zircon value)
pub const RETCODE_KILLED: i64 = -1024;
/// Return code of a process killed by a CPU exception (Zircon value)
pub const RETCODE_EXCEPTION: i64 = -1025;

static NEXT_KOID: AtomicU64 = AtomicU64::new(1024);

/// Kernel object ID: unique for the life of the system
pub fn new_koid() -> u64 {
    NEXT_KOID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------

pub struct Job {
    pub signals: SignalState,
    pub koid: u64,
    pub parent: Option<Arc<Job>>,
    children: SpinMutex<JobChildren>,
}

struct JobChildren {
    killed: bool,
    jobs: Vec<Weak<Job>>,
    processes: Vec<Weak<Process>>,
}

static ROOT_JOB: SpinMutex<Option<Arc<Job>>> = SpinMutex::new(None);

impl Job {
    fn make(parent: Option<Arc<Job>>) -> Arc<Self> {
        Arc::new(Self {
            signals: SignalState::new(0),
            koid: new_koid(),
            parent,
            children: SpinMutex::new(JobChildren { killed: false, jobs: Vec::new(), processes: Vec::new() }),
        })
    }

    /// The job every process started by the kernel belongs to
    pub fn root() -> Arc<Job> {
        ROOT_JOB.lock().get_or_insert_with(|| Job::make(None)).clone()
    }

    pub fn new_child(self: &Arc<Self>) -> Result<Arc<Job>, RxStatus> {
        let mut c = self.children.lock();
        if c.killed {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        let j = Job::make(Some(self.clone()));
        c.jobs.retain(|w| w.strong_count() > 0);
        c.jobs.push(Arc::downgrade(&j));
        Ok(j)
    }

    fn add_process(&self, p: &Arc<Process>) -> Result<(), RxStatus> {
        let mut c = self.children.lock();
        if c.killed {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        c.processes.retain(|w| w.strong_count() > 0);
        c.processes.push(Arc::downgrade(p));
        Ok(())
    }

    /// Mark this job and its child jobs killed (no new children) and return
    /// every live process in the tree
    pub fn kill_tree(&self) -> Vec<Arc<Process>> {
        let (jobs, mut procs) = {
            let mut c = self.children.lock();
            c.killed = true;
            let jobs: Vec<Arc<Job>> = c.jobs.iter().filter_map(|w| w.upgrade()).collect();
            let procs: Vec<Arc<Process>> = c.processes.iter().filter_map(|w| w.upgrade()).collect();
            (jobs, procs)
        };
        for j in jobs {
            procs.extend(j.kill_tree());
        }
        procs
    }
}

// ---------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcState {
    /// Created; no thread has been started
    Initial,
    Running,
    /// Killed or exiting; waiting for its threads to stop
    Dying,
    Dead,
}

struct ProcInner {
    state: ProcState,
    /// Scheduler IDs of the threads that have been started and not exited
    threads: Vec<u32>,
    return_code: i64,
    start_time: u64,
}

pub struct Process {
    pub signals: SignalState,
    pub koid: u64,
    pub job: Arc<Job>,
    /// Page table root (CR3) of the process's address space
    pub pml4: u64,
    pub aspace: Arc<Aspace>,
    pub handles: SpinMutex<HandleTable>,
    pub name: alloc::string::String,
    inner: SpinMutex<ProcInner>,
}

/// What a terminated process leaves behind, dropped by the caller once no
/// locks are held (dropping it may close other objects)
pub struct Remains {
    pub objects: Vec<KObject>,
    pub mappings: alloc::collections::BTreeMap<u64, super::vmar::Mapping>,
}

/// `object_get_info(ZX_INFO_PROCESS)`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InfoProcess {
    pub return_code: i64,
    pub start_time: i64,
    pub flags: u32,
    pub _pad: u32,
}
pub const INFO_PROCESS_STARTED: u32 = 1 << 0;
pub const INFO_PROCESS_EXITED: u32 = 1 << 1;

impl Process {
    pub fn new(job: &Arc<Job>, pml4: u64, name: &str) -> Result<Arc<Self>, RxStatus> {
        let p = Arc::new(Self {
            signals: SignalState::new(0),
            koid: new_koid(),
            job: job.clone(),
            pml4,
            aspace: Aspace::new(pml4),
            handles: SpinMutex::new(HandleTable::new()),
            name: alloc::string::String::from(name),
            inner: SpinMutex::new(ProcInner { state: ProcState::Initial, threads: Vec::new(), return_code: 0, start_time: 0 }),
        });
        job.add_process(&p)?;
        Ok(p)
    }

    pub fn state(&self) -> ProcState {
        self.inner.lock().state
    }

    /// Record a newly started thread. `first` is set by process_start,
    /// which may only be used once.
    pub fn add_thread(&self, tid: u32, first: bool) -> Result<(), RxStatus> {
        let mut i = self.inner.lock();
        match (i.state, first) {
            (ProcState::Initial, true) => {
                i.state = ProcState::Running;
                i.start_time = super::now_ns();
            }
            (ProcState::Running, false) => {}
            _ => return Err(RxStatus::ERR_BAD_STATE),
        }
        i.threads.push(tid);
        Ok(())
    }

    /// Check that a thread could be added (before allocating anything)
    pub fn can_add_thread(&self, first: bool) -> Result<(), RxStatus> {
        match (self.inner.lock().state, first) {
            (ProcState::Initial, true) | (ProcState::Running, false) => Ok(()),
            _ => Err(RxStatus::ERR_BAD_STATE),
        }
    }

    /// A thread exited; true if it was the last one (the caller then
    /// calls [`finalize`](Self::finalize))
    pub fn thread_exited(&self, tid: u32) -> bool {
        let mut i = self.inner.lock();
        i.threads.retain(|&t| t != tid);
        i.threads.is_empty() && matches!(i.state, ProcState::Running | ProcState::Dying)
    }

    /// Start killing the process: record the return code and return the
    /// threads that must exit (empty: call `finalize` now). None if it is
    /// already dying or dead.
    pub fn begin_kill(&self, code: i64) -> Option<Vec<u32>> {
        let mut i = self.inner.lock();
        match i.state {
            ProcState::Dying | ProcState::Dead => None,
            _ => {
                i.state = ProcState::Dying;
                i.return_code = code;
                Some(i.threads.clone())
            }
        }
    }

    /// Finish the process once no thread is left: close its handles, drop
    /// its mappings, signal TERMINATED. The returned remains are dropped by
    /// the caller with no locks held.
    pub fn finalize(&self) -> Option<Remains> {
        {
            let mut i = self.inner.lock();
            if i.state == ProcState::Dead {
                return None;
            }
            i.state = ProcState::Dead;
        }
        let objects = self.handles.lock().take_all();
        let mappings = self.aspace.destroy();
        self.signals.update(0, signals::TERMINATED);
        Some(Remains { objects, mappings })
    }

    pub fn info(&self) -> InfoProcess {
        let i = self.inner.lock();
        let mut flags = 0;
        if i.state != ProcState::Initial {
            flags |= INFO_PROCESS_STARTED;
        }
        if i.state == ProcState::Dead {
            flags |= INFO_PROCESS_EXITED;
        }
        InfoProcess { return_code: i.return_code, start_time: i.start_time as i64, flags, _pad: 0 }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // No thread can be running on these tables: each thread's table entry
        // holds a reference to this process until it has been reaped
        unsafe {
            crate::arch::amd64::mm::kernel_map::free_user_tables(self.pml4, |p| {
                crate::mm::pmm::pmm_free_page(p);
            })
        };
    }
}

// ---------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    New,
    Running,
    Dead,
}

pub struct Thread {
    pub signals: SignalState,
    pub koid: u64,
    pub process: Arc<Process>,
    inner: SpinMutex<(ThreadState, u32)>,
}

impl Thread {
    pub fn new(process: &Arc<Process>) -> Result<Arc<Self>, RxStatus> {
        if process.state() == ProcState::Dead {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        Ok(Arc::new(Self {
            signals: SignalState::new(0),
            koid: new_koid(),
            process: process.clone(),
            inner: SpinMutex::new((ThreadState::New, 0)),
        }))
    }

    pub fn state(&self) -> ThreadState {
        self.inner.lock().0
    }

    /// Scheduler ID once started
    pub fn tid(&self) -> Option<u32> {
        let i = self.inner.lock();
        (i.0 != ThreadState::New).then_some(i.1)
    }

    pub fn set_running(&self, tid: u32) -> Result<(), RxStatus> {
        let mut i = self.inner.lock();
        if i.0 != ThreadState::New {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        *i = (ThreadState::Running, tid);
        Ok(())
    }

    /// The thread has stopped for good
    pub fn mark_dead(&self) {
        self.inner.lock().0 = ThreadState::Dead;
        self.signals.update(0, signals::TERMINATED);
    }
}
