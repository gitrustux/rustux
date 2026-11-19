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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Who a process acts as (step 17): real, effective and saved user and
/// group ids, supplementary groups, and the file mode creation mask.
/// A new process gets a copy of its creator's. Filesystem and other
/// servers learn a request's sender from the message (`channel_read` with
/// `CHANNEL_READ_CREDS`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Creds {
    pub ruid: u32,
    pub euid: u32,
    pub suid: u32,
    pub rgid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub umask: u32,
    pub ngroups: u32,
    pub groups: [u32; MAX_GROUPS],
}

pub const MAX_GROUPS: usize = 16;

impl Creds {
    /// root: the first process, and what the kernel itself is
    pub const ROOT: Creds = Creds { ruid: 0, euid: 0, suid: 0, rgid: 0, egid: 0, sgid: 0, umask: 0o022, ngroups: 0, groups: [0; MAX_GROUPS] };

    pub fn is_root(&self) -> bool {
        self.euid == 0
    }

    /// May a process with these credentials change to `new`? root may
    /// change anything; others may only pick among their own real,
    /// effective and saved ids, keep their groups, and set the umask.
    pub fn may_become(&self, new: &Creds) -> bool {
        if self.is_root() {
            return true;
        }
        let u = |x: u32| x == self.ruid || x == self.euid || x == self.suid;
        let g = |x: u32| x == self.rgid || x == self.egid || x == self.sgid;
        u(new.ruid) && u(new.euid) && u(new.suid) && g(new.rgid) && g(new.egid) && g(new.sgid)
            && new.ngroups == self.ngroups && new.groups[..self.ngroups as usize] == self.groups[..self.ngroups as usize]
    }
}

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
    /// What its processes may not do (POLICY_*: FoxDen step FD2), taken
    /// from the parent when made; bits can be added, never removed
    policy: AtomicU32,
}

/// A sandboxed job (an app's: FOXDEN.md step FD2): no calls that reach
/// the system without a handle (firmware variables, changing one's own
/// credentials, the kernel's console) and none of the device, power and
/// clock calls (those need resources it never gets anyway; refused first)
pub const POLICY_SANDBOX: u32 = 1;

struct JobChildren {
    killed: bool,
    jobs: Vec<Weak<Job>>,
    processes: Vec<Weak<Process>>,
}

static ROOT_JOB: SpinMutex<Option<Arc<Job>>> = SpinMutex::new(None);

impl Job {
    fn make(parent: Option<Arc<Job>>) -> Arc<Self> {
        let policy = parent.as_ref().map_or(0, |p| p.policy());
        Arc::new(Self {
            signals: SignalState::new(0),
            koid: new_koid(),
            parent,
            children: SpinMutex::new(JobChildren { killed: false, jobs: Vec::new(), processes: Vec::new() }),
            policy: AtomicU32::new(policy),
        })
    }

    pub fn policy(&self) -> u32 {
        self.policy.load(Ordering::Acquire)
    }

    /// Add policy bits (never removes any). Jobs made under it later take
    /// them; set it before starting anything in it
    pub fn add_policy(&self, bits: u32) {
        self.policy.fetch_or(bits, Ordering::AcqRel);
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

    /// Live child jobs and processes
    pub fn children(&self) -> (Vec<Arc<Job>>, Vec<Arc<Process>>) {
        let c = self.children.lock();
        (c.jobs.iter().filter_map(|w| w.upgrade()).collect(), c.processes.iter().filter_map(|w| w.upgrade()).collect())
    }

    /// Every live process in the tree (nothing marked)
    pub fn processes_in_tree(&self) -> Vec<Arc<Process>> {
        let (jobs, mut procs) = self.children();
        for j in jobs {
            procs.extend(j.processes_in_tree());
        }
        procs
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
    /// I/O ports this process may use (`ioports_request`); None = none
    pub io_map: SpinMutex<Option<alloc::boxed::Box<[u8; crate::arch::amd64::descriptor::IO_MAP_BYTES]>>>,
    /// CPU time used by all its threads, ns
    pub runtime_ns: AtomicU64,
    /// Who it acts as (step 17)
    pub creds: SpinMutex<Creds>,
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
    pub fn new(job: &Arc<Job>, pml4: u64, name: &str, creds: Creds) -> Result<Arc<Self>, RxStatus> {
        let p = Arc::new(Self {
            signals: SignalState::new(0),
            koid: new_koid(),
            job: job.clone(),
            pml4,
            aspace: Aspace::new(pml4),
            handles: SpinMutex::new(HandleTable::new()),
            name: alloc::string::String::from(name),
            io_map: SpinMutex::new(None),
            runtime_ns: AtomicU64::new(0),
            creds: SpinMutex::new(creds),
            inner: SpinMutex::new(ProcInner { state: ProcState::Initial, threads: Vec::new(), return_code: 0, start_time: 0 }),
        });
        job.add_process(&p)?;
        Ok(p)
    }

    pub fn state(&self) -> ProcState {
        self.inner.lock().state
    }

    /// Scheduler ids of its running threads
    pub fn thread_ids(&self) -> Vec<u32> {
        self.inner.lock().threads.clone()
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
    /// its mappings, signal TERMINATED. Call with no locks held. As in
    /// Zircon, the handles are closed before TERMINATED is raised, so a
    /// waiter that sees TERMINATED also sees PEER_CLOSED on the channels the
    /// process held (`ipctest` caught this on 4 CPUs).
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
        drop(objects);
        drop(mappings);
        self.signals.update(0, signals::TERMINATED);
        Some(Remains { objects: Vec::new(), mappings: Default::default() })
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
    /// CPU time used, ns
    pub runtime_ns: AtomicU64,
    /// User FS base (thread-local storage), loaded when the thread runs
    pub fs_base: AtomicU64,
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
            runtime_ns: AtomicU64::new(0),
            fs_base: AtomicU64::new(0),
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

    /// Add CPU time (scheduler)
    pub fn charge(&self, ns: u64) {
        self.runtime_ns.fetch_add(ns, Ordering::Relaxed);
        self.process.runtime_ns.fetch_add(ns, Ordering::Relaxed);
    }

    /// The thread has stopped for good
    pub fn mark_dead(&self) {
        self.inner.lock().0 = ThreadState::Dead;
        self.signals.update(0, signals::TERMINATED);
    }
}
