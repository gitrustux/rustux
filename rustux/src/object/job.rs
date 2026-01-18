// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Job Objects
//!
//! Jobs are containers for processes and other jobs, forming a hierarchical
//! tree structure. They are used for resource accounting and policy enforcement.
//!
//! # Design
//!
//! - **Hierarchical**: Jobs form a tree with a single root job
//! - **Policy**: Jobs can enforce CPU, memory, and job policies
//! - **Accounting**: Track resource usage across all child processes
//! - **Lifecycle**: Jobs are created explicitly and destroyed when all children exit
//!
//! # Usage
//!
//! ```rust
//! let root_job = Job::new_root();
//! let child_job = Job::new_child(&root_job, 0)?;
//! ```

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::SpinMutex;
use crate::object::handle::{KernelObjectBase, ObjectType};

/// ============================================================================
/// Job ID
/// ============================================================================

/// Job identifier
pub type JobId = u64;

/// Invalid job ID
pub const JOB_ID_INVALID: JobId = 0;

/// Root job ID
pub const JOB_ID_ROOT: JobId = 1;

/// Next job ID counter
static mut NEXT_JOB_ID: AtomicU64 = AtomicU64::new(JOB_ID_ROOT + 1);

/// Allocate a new job ID
fn alloc_job_id() -> JobId {
    unsafe { NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed) }
}

/// ============================================================================
/// Job Policy
/// ============================================================================

/// Job policy for controlling child process behavior
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPolicy {
    /// No special policy
    None = 0,

    /// Basic policy (minimal restrictions)
    Basic = 1,

    /// Restrict VMO creation (no new VMOs)
    NoNewVmos = 1 << 1,

    /// Restrict channel creation
    NoNewChannels = 1 << 2,

    /// Restrict event creation
    NoNewEvents = 1 << 3,

    /// Restrict socket creation
    NoNewSockets = 1 << 4,

    /// Restrict process creation
    NoNewProcesses = 1 << 5,

    /// Restrict thread creation
    NoNewThreads = 1 << 6,

    /// Kill all processes when job is closed
    KillOnClose = 1 << 7,

    /// Allow profiling
    AllowProfile = 1 << 8,

    /// Allow debugging
    AllowDebug = 1 << 9,
}

impl JobPolicy {
    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Basic,
            _ => Self::None,
        }
    }

    /// Convert to raw flags
    pub fn to_flags(self) -> u32 {
        self as u32
    }

    /// Check if policy contains another policy
    pub fn contains(self, other: Self) -> bool {
        (self.to_flags() & other.to_flags()) != 0
    }
}

impl core::ops::BitOr for JobPolicy {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self::from_raw(self.to_flags() | rhs.to_flags())
    }
}

/// ============================================================================
/// Resource Limits
/// ============================================================================

/// Resource limits for a job
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Maximum memory in bytes (0 = no limit)
    pub max_memory: u64,

    /// Maximum CPU time (0 = no limit)
    pub max_cpu_time: u64,

    /// Maximum number of processes (0 = no limit)
    pub max_processes: u64,

    /// Maximum number of threads (0 = no limit)
    pub max_threads: u64,

    /// Maximum number of jobs (0 = no limit)
    pub max_jobs: u64,
}

impl ResourceLimits {
    /// Create unlimited resource limits
    pub const fn unlimited() -> Self {
        Self {
            max_memory: 0,
            max_cpu_time: 0,
            max_processes: 0,
            max_threads: 0,
            max_jobs: 0,
        }
    }

    /// Check if memory is limited
    pub const fn has_memory_limit(self) -> bool {
        self.max_memory > 0
    }

    /// Check if CPU time is limited
    pub const fn has_cpu_time_limit(self) -> bool {
        self.max_cpu_time > 0
    }
}

/// ============================================================================
/// Job Statistics
/// ============================================================================

/// Job resource usage statistics
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JobStats {
    /// Current memory usage in bytes
    pub memory_usage: u64,

    /// Current CPU time (in nanoseconds)
    pub cpu_time: u64,

    /// Number of processes
    pub process_count: u64,

    /// Number of threads
    pub thread_count: u64,

    /// Number of child jobs
    pub job_count: u64,
}

impl JobStats {
    /// Create zero statistics
    pub const fn zero() -> Self {
        Self {
            memory_usage: 0,
            cpu_time: 0,
            process_count: 0,
            thread_count: 0,
            job_count: 0,
        }
    }
}

/// ============================================================================
/// Job
/// ============================================================================

/// Job object
///
/// Jobs are containers for processes and other jobs.
pub struct Job {
    /// Kernel object base
    pub base: KernelObjectBase,

    /// Job ID
    pub id: JobId,

    /// Parent job ID
    pub parent_id: SpinMutex<Option<JobId>>,

    /// Child job IDs
    pub children: SpinMutex<alloc::vec::Vec<JobId>>,

    /// Process IDs in this job
    pub processes: SpinMutex<alloc::vec::Vec<u64>>,

    /// Job policy
    pub policy: SpinMutex<JobPolicy>,

    /// Resource limits
    pub limits: SpinMutex<ResourceLimits>,

    /// Resource usage statistics
    pub stats: SpinMutex<JobStats>,
}

impl Job {
    /// Create the root job
    pub fn new_root() -> Self {
        Self {
            base: KernelObjectBase::new(ObjectType::Job),
            id: JOB_ID_ROOT,
            parent_id: SpinMutex::new(None),
            children: SpinMutex::new(alloc::vec::Vec::new()),
            processes: SpinMutex::new(alloc::vec::Vec::new()),
            policy: SpinMutex::new(JobPolicy::Basic),
            limits: SpinMutex::new(ResourceLimits::unlimited()),
            stats: SpinMutex::new(JobStats::zero()),
        }
    }

    /// Create a new child job
    ///
    /// # Arguments
    ///
    /// * `parent` - Parent job
    /// * `policy` - Job policy flags
    pub fn new_child(parent: &Job, policy: u32) -> Result<Self, &'static str> {
        let child = Self {
            base: KernelObjectBase::new(ObjectType::Job),
            id: alloc_job_id(),
            parent_id: SpinMutex::new(Some(parent.id)),
            children: SpinMutex::new(alloc::vec::Vec::new()),
            processes: SpinMutex::new(alloc::vec::Vec::new()),
            policy: SpinMutex::new(JobPolicy::from_raw(policy)),
            limits: SpinMutex::new(ResourceLimits::unlimited()),
            stats: SpinMutex::new(JobStats::zero()),
        };

        // Add to parent's children
        parent.children.lock().push(child.id);

        Ok(child)
    }

    /// Get job ID
    pub const fn id(&self) -> JobId {
        self.id
    }

    /// Get parent job ID
    pub fn parent_id(&self) -> Option<JobId> {
        *self.parent_id.lock()
    }

    /// Get job policy
    pub fn policy(&self) -> JobPolicy {
        *self.policy.lock()
    }

    /// Set job policy
    pub fn set_policy(&self, policy: JobPolicy) {
        *self.policy.lock() = policy;
    }

    /// Get resource limits
    pub fn limits(&self) -> ResourceLimits {
        *self.limits.lock()
    }

    /// Set resource limits
    pub fn set_limits(&self, limits: ResourceLimits) {
        *self.limits.lock() = limits;
    }

    /// Get job statistics
    pub fn stats(&self) -> JobStats {
        *self.stats.lock()
    }

    /// Add a child job
    pub fn add_child(&self, child_id: JobId) {
        self.children.lock().push(child_id);
    }

    /// Remove a child job
    pub fn remove_child(&self, child_id: JobId) {
        let mut children = self.children.lock();
        if let Some(pos) = children.iter().position(|&id| id == child_id) {
            children.remove(pos);
        }
    }

    /// Add a process
    pub fn add_process(&self, process_id: u64) {
        self.processes.lock().push(process_id);
        self.stats.lock().process_count += 1;
    }

    /// Remove a process
    pub fn remove_process(&self, process_id: u64) {
        let mut processes = self.processes.lock();
        if let Some(pos) = processes.iter().position(|&id| id == process_id) {
            processes.remove(pos);
            self.stats.lock().process_count -= 1;
        }
    }

    /// Get child count
    pub fn child_count(&self) -> usize {
        self.children.lock().len()
    }

    /// Get process count
    pub fn process_count(&self) -> usize {
        self.processes.lock().len()
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
    fn test_job_root() {
        let root = Job::new_root();

        assert_eq!(root.id(), JOB_ID_ROOT);
        assert_eq!(root.parent_id(), None);
        assert_eq!(root.child_count(), 0);
        assert_eq!(root.process_count(), 0);
    }

    #[test]
    fn test_job_child() {
        let root = Job::new_root();
        let child = Job::new_child(&root, 0).unwrap();

        assert_eq!(root.child_count(), 1);
        assert_eq!(child.parent_id(), Some(root.id()));
    }

    #[test]
    fn test_job_policy() {
        let policy = JobPolicy::NoNewProcesses | JobPolicy::NoNewThreads;

        assert!(policy.contains(JobPolicy::NoNewProcesses));
        assert!(policy.contains(JobPolicy::NoNewThreads));
        assert!(!policy.contains(JobPolicy::NoNewChannels));
    }

    #[test]
    fn test_job_processes() {
        let job = Job::new_root();

        job.add_process(1);
        job.add_process(2);

        assert_eq!(job.process_count(), 2);

        job.remove_process(1);
        assert_eq!(job.process_count(), 1);
    }

    #[test]
    fn test_job_children() {
        let root = Job::new_root();

        let child1 = Job::new_child(&root, 0).unwrap();
        let child2 = Job::new_child(&root, 0).unwrap();

        assert_eq!(root.child_count(), 2);

        root.remove_child(child1.id());
        assert_eq!(root.child_count(), 1);
    }

    #[test]
    fn test_resource_limits() {
        let limits = ResourceLimits::unlimited();

        assert!(!limits.has_memory_limit());
        assert!(!limits.has_cpu_time_limit());

        let limits = ResourceLimits {
            max_memory: 1024 * 1024,
            ..ResourceLimits::unlimited()
        };

        assert!(limits.has_memory_limit());
    }

    #[test]
    fn test_job_stats() {
        let stats = JobStats::zero();

        assert_eq!(stats.memory_usage, 0);
        assert_eq!(stats.process_count, 0);
        assert_eq!(stats.thread_count, 0);
    }
}
