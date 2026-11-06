// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Task syscalls: jobs, processes, threads. Signatures are in
//! docs/SYSCALL.md.

use super::object::{install, lookup, ret, take, R};
use super::{user, SyscallArgs, SyscallRet};
use crate::arch::amd64::mm::RxStatus;
use crate::kobj::task::{self, Job, Process, Thread};
use crate::kobj::{KObject, Rights, Vmar};
use crate::process::{launch, run};
use alloc::sync::Arc;

fn typed<T>(h: u32, right: u32, pick: fn(KObject) -> Option<T>) -> R<T> {
    let (obj, rights) = lookup(h, 0)?;
    let t = pick(obj).ok_or(RxStatus::ERR_WRONG_TYPE)?;
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(t)
}

fn process_of(h: u32, right: u32) -> R<Arc<Process>> {
    typed(h, right, |o| if let KObject::Process(p) = o { Some(p) } else { None })
}
fn thread_of(h: u32, right: u32) -> R<Arc<Thread>> {
    typed(h, right, |o| if let KObject::Thread(t) = o { Some(t) } else { None })
}
fn job_of(h: u32, right: u32) -> R<Arc<Job>> {
    typed(h, right, |o| if let KObject::Job(j) = o { Some(j) } else { None })
}

/// Object name from user memory (up to 32 bytes, invalid UTF-8 replaced)
fn read_name(ptr: usize, len: usize) -> R<alloc::string::String> {
    let bytes = user::read_bytes(ptr, len.min(32))?;
    Ok(alloc::string::String::from_utf8_lossy(&bytes).into_owned())
}

/// process_create(job, *name, name_len, options, *proc_out, *vmar_out)
pub fn sys_process_create(args: SyscallArgs) -> SyscallRet {
    let (jh, name, name_len, options) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    let (out_p, out_v) = (args.arg(4), args.arg(5));
    ret((|| {
        let job = job_of(jh, Rights::MANAGE_PROCESS)?;
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let name = read_name(name, name_len)?;
        user::check(out_p, 4, true)?;
        user::check(out_v, 4, true)?;
        let process = launch::new_process(&job, &name)?;
        let vmar = Vmar::root(process.aspace.clone());
        let hp = install(KObject::Process(process), Rights::PROCESS)?;
        let hv = match install(KObject::Vmar(vmar), Rights::VMAR) {
            Ok(h) => h,
            Err(e) => {
                let _ = take(hp);
                return Err(e);
            }
        };
        user::write_u32(out_p, hp)?;
        user::write_u32(out_v, hv)?;
        Ok(0)
    })())
}

/// process_start(process, thread, entry, stack, arg1_handle, arg2)
///
/// `arg1_handle` (0 for none) is moved into the new process; the thread
/// finds its new value in RDI and `arg2` in RSI.
pub fn sys_process_start(args: SyscallArgs) -> SyscallRet {
    let (ph, th, entry, stack) = (args.arg_u32(0), args.arg_u32(1), args.arg_u64(2), args.arg_u64(3));
    let (a1, a2) = (args.arg_u32(4), args.arg_u64(5));
    ret((|| {
        // Like Zircon, arg1 is consumed whether or not the call succeeds
        let moved = if a1 != 0 { Some(take(a1)?) } else { None };
        let process = process_of(ph, Rights::WRITE)?;
        let thread = thread_of(th, Rights::WRITE)?;
        if !Arc::ptr_eq(&thread.process, &process) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        process.can_add_thread(true)?;
        let mut value = 0;
        if let Some((obj, rights)) = moved {
            if !rights.has(Rights::TRANSFER) {
                return Err(RxStatus::ERR_ACCESS_DENIED);
            }
            value = process.handles.lock().add(obj, rights).ok_or(RxStatus::ERR_NO_MEMORY)?;
        }
        if let Err(e) = launch::start_thread(&thread, true, entry, stack, value as u64, a2) {
            let gone = process.handles.lock().remove(value);
            drop(gone);
            return Err(e);
        }
        Ok(0)
    })())
}

/// thread_create(process, *name, name_len, options, *out)
pub fn sys_thread_create(args: SyscallArgs) -> SyscallRet {
    let (ph, name, name_len, options, out) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3), args.arg(4));
    ret((|| {
        let process = process_of(ph, Rights::MANAGE_THREAD)?;
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let _ = read_name(name, name_len)?;
        user::check(out, 4, true)?;
        let t = Thread::new(&process)?;
        let v = install(KObject::Thread(t), Rights::THREAD)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// thread_start(thread, entry, stack, arg1, arg2) — another thread in a
/// process that is already running
pub fn sys_thread_start(args: SyscallArgs) -> SyscallRet {
    let (th, entry, stack, a1, a2) = (args.arg_u32(0), args.arg_u64(1), args.arg_u64(2), args.arg_u64(3), args.arg_u64(4));
    ret((|| {
        let thread = thread_of(th, Rights::MANAGE_THREAD)?;
        launch::start_thread(&thread, false, entry, stack, a1, a2)?;
        Ok(0)
    })())
}

/// thread_exit() — never returns
pub fn sys_thread_exit(_args: SyscallArgs) -> SyscallRet {
    run::exit_thread()
}

/// process_exit(return_code) — never returns
pub fn sys_process_exit(args: SyscallArgs) -> SyscallRet {
    run::exit_current(args.arg_i64(0))
}

fn kill_thread(t: Arc<Thread>) {
    let mine = run::current_thread().map_or(false, |(_, me)| Arc::ptr_eq(&me, &t));
    if mine {
        drop(t);
        run::exit_thread();
    }
    match t.tid() {
        Some(tid) if t.state() != task::ThreadState::Dead => run::kill(tid),
        None => t.mark_dead(),
        _ => {}
    }
}

/// task_kill(handle) — a job, process or thread; needs DESTROY. Killing
/// the caller's own process (or a job containing it) does not return.
///
/// Every reference is moved into the call that may not return, so none is
/// left on this kernel stack.
pub fn sys_task_kill(args: SyscallArgs) -> SyscallRet {
    let h = args.arg_u32(0);
    let (obj, rights) = match lookup(h, 0) {
        Ok(v) => v,
        Err(e) => return ret(Err(e)),
    };
    if !matches!(obj, KObject::Process(_) | KObject::Thread(_) | KObject::Job(_)) {
        return ret(Err(RxStatus::ERR_WRONG_TYPE));
    }
    if !rights.has(Rights::DESTROY) {
        return ret(Err(RxStatus::ERR_ACCESS_DENIED));
    }
    match obj {
        KObject::Process(p) => run::kill_process(p, task::RETCODE_KILLED),
        KObject::Thread(t) => kill_thread(t),
        KObject::Job(j) => {
            let me = run::current_thread().map(|(_, t)| t.process.clone());
            let mut mine = None;
            for p in j.kill_tree() {
                if me.as_ref().map_or(false, |m| Arc::ptr_eq(&p, m)) {
                    mine = Some(p);
                } else {
                    run::kill_process(p, task::RETCODE_KILLED);
                }
            }
            drop(me);
            drop(j);
            if let Some(p) = mine {
                run::kill_process(p, task::RETCODE_KILLED);
            }
        }
        _ => {}
    }
    ret(Ok(0))
}

/// object_get_info topics (Zircon values)
// object_get_info topics (Zircon values)
const INFO_HANDLE_BASIC: u32 = 2;
const INFO_PROCESS: u32 = 3;
const INFO_PROCESS_THREADS: u32 = 4;
const INFO_JOB_CHILDREN: u32 = 6;
const INFO_JOB_PROCESSES: u32 = 7;
const INFO_TASK_STATS: u32 = 11;
const INFO_TASK_RUNTIME: u32 = 25;

/// The thread objects of a process that are running
fn threads_of(p: &Process) -> alloc::vec::Vec<Arc<Thread>> {
    let tids = p.thread_ids();
    let table = crate::process::table::PROCESS_TABLE.lock();
    tids.iter().filter_map(|&t| table.get(t).map(|e| e.thread.clone())).collect()
}

/// Koid of a task object (0 for other objects, which have none yet)
fn koid_of(o: &KObject) -> (u64, u64) {
    match o {
        KObject::Job(j) => (j.koid, j.parent.as_ref().map_or(0, |p| p.koid)),
        KObject::Process(p) => (p.koid, p.job.koid),
        KObject::Thread(t) => (t.koid, t.process.koid),
        _ => (0, 0),
    }
}

/// object_get_info(handle, topic, *buf, size, *actual, *avail). Topics:
/// HANDLE_BASIC (2), PROCESS (3), PROCESS_THREADS (4), JOB_CHILDREN (6),
/// JOB_PROCESSES (7), TASK_STATS (11), TASK_RUNTIME (25). For the lists,
/// `actual` is how many koids were written and `avail` how many exist.
pub fn sys_object_get_info(args: SyscallArgs) -> SyscallRet {
    let (h, topic, buf, size) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    let (actual, avail) = (args.arg(4), args.arg(5));
    ret((|| {
        let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let (mut n_actual, mut n_avail) = (1u64, 1u64);
        let push = |out: &mut alloc::vec::Vec<u8>, v: u64| out.extend_from_slice(&v.to_le_bytes());
        // A list of koids: as many as fit
        let list = |out: &mut alloc::vec::Vec<u8>, koids: &[u64], na: &mut u64, nv: &mut u64| {
            let fit = (size / 8).min(koids.len());
            for k in &koids[..fit] {
                out.extend_from_slice(&k.to_le_bytes());
            }
            *na = fit as u64;
            *nv = koids.len() as u64;
        };
        match topic {
            INFO_HANDLE_BASIC => {
                let (obj, rights) = lookup(h, 0)?;
                let (koid, related) = koid_of(&obj);
                push(&mut out, koid);
                out.extend_from_slice(&rights.0.to_le_bytes());
                out.extend_from_slice(&(obj.obj_type() as u32).to_le_bytes());
                push(&mut out, related);
            }
            INFO_PROCESS => {
                let p = process_of(h, Rights::INSPECT)?;
                let info = p.info();
                let n = core::mem::size_of::<task::InfoProcess>();
                out.extend_from_slice(unsafe { core::slice::from_raw_parts(&info as *const _ as *const u8, n) });
            }
            INFO_PROCESS_THREADS => {
                let p = process_of(h, Rights::ENUMERATE)?;
                let koids: alloc::vec::Vec<u64> = threads_of(&p).iter().map(|t| t.koid).collect();
                list(&mut out, &koids, &mut n_actual, &mut n_avail);
            }
            INFO_JOB_CHILDREN | INFO_JOB_PROCESSES => {
                let j = job_of(h, Rights::ENUMERATE)?;
                let (jobs, procs) = j.children();
                let koids: alloc::vec::Vec<u64> = if topic == INFO_JOB_CHILDREN {
                    jobs.iter().map(|c| c.koid).collect()
                } else {
                    procs.iter().filter(|p| p.state() != task::ProcState::Dead).map(|p| p.koid).collect()
                };
                list(&mut out, &koids, &mut n_actual, &mut n_avail);
            }
            INFO_TASK_STATS => {
                // { mapped, private, shared, scaled shared (not computed: 0) } bytes
                let p = process_of(h, Rights::INSPECT)?;
                let (mapped, private, shared) = p.aspace.stats();
                for v in [mapped, private, shared, 0] {
                    push(&mut out, v);
                }
            }
            INFO_TASK_RUNTIME => {
                // { cpu_time, queue_time, page_fault_time, lock_contention_time } ns
                let (obj, rights) = lookup(h, 0)?;
                if !rights.has(Rights::INSPECT) {
                    return Err(RxStatus::ERR_ACCESS_DENIED);
                }
                let cpu = match obj {
                    KObject::Process(p) => p.runtime_ns.load(core::sync::atomic::Ordering::Relaxed),
                    KObject::Thread(t) => t.runtime_ns.load(core::sync::atomic::Ordering::Relaxed),
                    _ => return Err(RxStatus::ERR_WRONG_TYPE),
                };
                for v in [cpu, 0, 0, 0] {
                    push(&mut out, v);
                }
            }
            _ => return Err(RxStatus::ERR_NOT_SUPPORTED),
        }
        let is_list = matches!(topic, INFO_PROCESS_THREADS | INFO_JOB_CHILDREN | INFO_JOB_PROCESSES);
        if !is_list && size < out.len() {
            return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
        }
        if !out.is_empty() {
            user::write_bytes(buf, &out)?;
        }
        if actual != 0 {
            user::write_u64(actual, n_actual)?;
        }
        if avail != 0 {
            user::write_u64(avail, n_avail)?;
        }
        Ok(0)
    })())
}

/// object_get_child(handle, koid, rights, *out) — a job's child job or
/// process, or a process's thread, by koid
pub fn sys_object_get_child(args: SyscallArgs) -> SyscallRet {
    let (h, koid, rights, out) = (args.arg_u32(0), args.arg(1) as u64, args.arg_u32(2), args.arg(3));
    ret((|| {
        user::check(out, 4, true)?;
        let (obj, have) = lookup(h, 0)?;
        if !have.has(Rights::ENUMERATE) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        let (child, base) = match obj {
            KObject::Job(j) => {
                let (jobs, procs) = j.children();
                if let Some(c) = jobs.into_iter().find(|c| c.koid == koid) {
                    (KObject::Job(c), Rights::JOB)
                } else if let Some(p) = procs.into_iter().find(|p| p.koid == koid) {
                    (KObject::Process(p), Rights::PROCESS)
                } else {
                    return Err(RxStatus::ERR_NOT_FOUND);
                }
            }
            KObject::Process(p) => {
                let t = threads_of(&p).into_iter().find(|t| t.koid == koid).ok_or(RxStatus::ERR_NOT_FOUND)?;
                (KObject::Thread(t), Rights::THREAD)
            }
            _ => return Err(RxStatus::ERR_WRONG_TYPE),
        };
        // No more rights than the parent handle has
        let want = if rights & Rights::SAME_RIGHTS != 0 { base } else { rights };
        if want & !base != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let v = install(child, want)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// object_set_property(handle, property, *buf, len) — only REGISTER_FS
/// (4) of a thread: its FS base, a u64 user address (thread-local storage)
pub fn sys_object_set_property(args: SyscallArgs) -> SyscallRet {
    let (h, prop, buf, len) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    ret((|| {
        const PROP_REGISTER_FS: u32 = 4;
        let t = thread_of(h, Rights::SET_PROPERTY)?;
        if prop != PROP_REGISTER_FS {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        if len != 8 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let b = user::read_bytes(buf, 8)?;
        let v = u64::from_le_bytes(b[..8].try_into().unwrap());
        // A canonical user address (or 0)
        if v >= crate::kobj::vmar::ROOT_END {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        t.fs_base.store(v, core::sync::atomic::Ordering::Relaxed);
        // Our own thread: load it now (others load it when they next run)
        if let Some((_, cur)) = run::current_thread() {
            if Arc::ptr_eq(&cur, &t) {
                run::set_fs_base(v);
            }
        }
        Ok(0)
    })())
}

/// object_get_property(handle, property, *buf, len) — only NAME (3), of a
/// process (a thread's name is its process's)
pub fn sys_object_get_property(args: SyscallArgs) -> SyscallRet {
    let (h, prop, buf, len) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    ret((|| {
        const PROP_NAME: u32 = 3;
        let (obj, rights) = lookup(h, 0)?;
        if !rights.has(Rights::GET_PROPERTY) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if prop != PROP_NAME {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        let name = match &obj {
            KObject::Process(p) => p.name.clone(),
            KObject::Thread(t) => t.process.name.clone(),
            _ => alloc::string::String::new(),
        };
        // NUL-terminated, cut to fit (Zircon names are at most 31 bytes)
        let mut bytes: alloc::vec::Vec<u8> = name.into_bytes();
        bytes.truncate(len.saturating_sub(1).min(31));
        bytes.push(0);
        if len == 0 {
            return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
        }
        user::write_bytes(buf, &bytes)?;
        Ok(0)
    })())
}

/// job_create(parent_job, options, *out)
pub fn sys_job_create(args: SyscallArgs) -> SyscallRet {
    let (ph, options, out) = (args.arg_u32(0), args.arg(1), args.arg(2));
    ret((|| {
        let parent = job_of(ph, Rights::MANAGE_JOB)?;
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Job(parent.new_child()?), Rights::JOB)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// handle_replace(handle, rights, *out) — a new handle (same object, the
/// given rights or SAME_RIGHTS) replaces the old one, which is closed even
/// on failure
pub fn sys_handle_replace(args: SyscallArgs) -> SyscallRet {
    let (h, want, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2));
    ret((|| {
        user::check(out, 4, true)?;
        let (obj, rights) = take(h)?;
        let new = if want == Rights::SAME_RIGHTS {
            rights.0
        } else if want & !rights.0 != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        } else {
            want
        };
        let v = install(obj, new)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

