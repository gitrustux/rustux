// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Task syscalls: jobs, processes, threads. Signatures are in
//! docs/SYSCALL.md.

use super::object::{current_process, install, lookup, ret, take, R};
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
    crate::kobj::debugcon("[EXIT]\n");
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
const INFO_PROCESS: u32 = 3;

/// object_get_info(handle, topic, *buffer, buffer_size, *actual, *avail)
/// Only ZX_INFO_PROCESS (3) for now.
pub fn sys_object_get_info(args: SyscallArgs) -> SyscallRet {
    let (h, topic, buf, size) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    let (actual, avail) = (args.arg(4), args.arg(5));
    ret((|| {
        match topic {
            INFO_PROCESS => {
                let p = process_of(h, Rights::INSPECT)?;
                let info = p.info();
                let n = core::mem::size_of::<task::InfoProcess>();
                if size < n {
                    return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
                }
                let bytes = unsafe { core::slice::from_raw_parts(&info as *const _ as *const u8, n) };
                user::write_bytes(buf, bytes)?;
            }
            _ => return Err(RxStatus::ERR_NOT_SUPPORTED),
        }
        for out in [actual, avail] {
            if out != 0 {
                user::write_u64(out, 1)?;
            }
        }
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

/// spawn(*path) — start /bin/... from the ramdisk in the caller's job.
/// Returns the new thread's id (shown as a pid).
pub fn sys_spawn(args: SyscallArgs) -> SyscallRet {
    spawn(args.arg(0), 0)
}

/// spawn_process(*path, *process_out) — like spawn, and writes a process
/// handle to `process_out` (wait on it for TERMINATED)
pub fn sys_spawn_process(args: SyscallArgs) -> SyscallRet {
    match args.arg(1) {
        0 => ret(Err(RxStatus::ERR_INVALID_ARGS)),
        out => spawn(args.arg(0), out),
    }
}

fn spawn(path: usize, out: usize) -> SyscallRet {
    ret((|| {
        let path = user::read_cstr(path, 256)?;
        if out != 0 {
            user::check(out, 4, true)?;
        }
        let job = current_process()?.job.clone();
        let (process, tid) = launch::spawn_elf(&path, &job)?;
        if out != 0 {
            let v = install(KObject::Process(process), Rights::PROCESS)?;
            user::write_u32(out, v)?;
        }
        Ok(tid as usize)
    })())
}
