// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Object syscalls: handles, events, event pairs, signals, waiting, timers.
//! Signatures are in docs/SYSCALL.md.

use super::{err_to_ret, ok_to_ret, user, SyscallArgs, SyscallRet};
use crate::arch::amd64::mm::RxStatus;
use crate::kobj::{self, signals, Event, EventPair, KObject, Rights, Timer};
use crate::process::run;
use crate::process::table::PROCESS_TABLE;
use alloc::vec::Vec;

type R<T> = Result<T, RxStatus>;

fn ret(r: R<usize>) -> SyscallRet {
    match r {
        Ok(v) => ok_to_ret(v),
        Err(e) => err_to_ret(e),
    }
}

fn current_pid() -> R<u32> {
    PROCESS_TABLE.lock().current_pid().ok_or(RxStatus::ERR_BAD_STATE)
}

/// Look up a handle in the current process and check its rights
pub fn lookup(handle: u32, required: u32) -> R<(KObject, Rights)> {
    let table = PROCESS_TABLE.lock();
    let p = table.current_pid().and_then(|c| table.get(c)).ok_or(RxStatus::ERR_BAD_STATE)?;
    let (obj, rights) = p.handles.get(handle).ok_or(RxStatus::ERR_BAD_HANDLE)?;
    if !rights.has(required) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok((obj, rights))
}

/// Add a handle to the current process
pub fn install(obj: KObject, rights: u32) -> R<u32> {
    let mut table = PROCESS_TABLE.lock();
    let cur = table.current_pid().ok_or(RxStatus::ERR_BAD_STATE)?;
    let p = table.get_mut(cur).ok_or(RxStatus::ERR_BAD_STATE)?;
    p.handles.add(obj, Rights(rights)).ok_or(RxStatus::ERR_NO_MEMORY)
}

/// Remove a handle from the current process (the object is returned so it
/// is dropped after the process table lock is released)
fn take(handle: u32) -> R<(KObject, Rights)> {
    let mut table = PROCESS_TABLE.lock();
    let cur = table.current_pid().ok_or(RxStatus::ERR_BAD_STATE)?;
    let p = table.get_mut(cur).ok_or(RxStatus::ERR_BAD_STATE)?;
    p.handles.remove(handle).ok_or(RxStatus::ERR_BAD_HANDLE)
}

// ---------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------

/// handle_close(handle) — closing 0 is a no-op
pub fn sys_handle_close(args: SyscallArgs) -> SyscallRet {
    let h = args.arg_u32(0);
    if h == 0 {
        return ok_to_ret(0);
    }
    ret(take(h).map(|_obj| 0))
}

/// handle_duplicate(handle, rights, *out) — rights must be a subset of
/// the original's, or SAME_RIGHTS
pub fn sys_handle_duplicate(args: SyscallArgs) -> SyscallRet {
    let (h, want, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2));
    ret((|| {
        user::check(out, 4, true)?;
        let (obj, rights) = lookup(h, Rights::DUPLICATE)?;
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

// ---------------------------------------------------------------------
// Events, event pairs, signals
// ---------------------------------------------------------------------

/// event_create(options, *out)
pub fn sys_event_create(args: SyscallArgs) -> SyscallRet {
    let out = args.arg(1);
    ret((|| {
        user::check(out, 4, true)?;
        let v = install(KObject::Event(Event::new()), Rights::EVENT)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// eventpair_create(options, *out0, *out1)
pub fn sys_eventpair_create(args: SyscallArgs) -> SyscallRet {
    let (out0, out1) = (args.arg(1), args.arg(2));
    ret((|| {
        user::check(out0, 4, true)?;
        user::check(out1, 4, true)?;
        let (a, b) = EventPair::new_pair();
        let h0 = install(KObject::EventPair(a), Rights::EVENTPAIR)?;
        let h1 = match install(KObject::EventPair(b), Rights::EVENTPAIR) {
            Ok(h) => h,
            Err(e) => {
                let _ = take(h0);
                return Err(e);
            }
        };
        user::write_u32(out0, h0)?;
        user::write_u32(out1, h1)?;
        Ok(0)
    })())
}

/// object_signal(handle, clear_mask, set_mask) — on the object itself
pub fn sys_object_signal(args: SyscallArgs) -> SyscallRet {
    let (h, clear, set) = (args.arg_u32(0), args.arg_u32(1), args.arg_u32(2));
    ret((|| {
        let (obj, _) = lookup(h, Rights::SIGNAL)?;
        if (clear | set) & !obj.user_settable() != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        obj.signal_state().update(clear, set);
        Ok(0)
    })())
}

/// object_signal_peer(handle, clear_mask, set_mask) — on the other end of
/// an event pair
pub fn sys_object_signal_peer(args: SyscallArgs) -> SyscallRet {
    let (h, clear, set) = (args.arg_u32(0), args.arg_u32(1), args.arg_u32(2));
    ret((|| {
        let (obj, rights) = lookup(h, 0)?;
        let KObject::EventPair(e) = &obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        if !rights.has(Rights::SIGNAL_PEER) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        let peer = e.peer().ok_or(RxStatus::ERR_PEER_CLOSED)?;
        if (clear | set) & !(signals::USER_ALL | signals::SIGNALED) != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        peer.signals.update(clear, set);
        Ok(0)
    })())
}

// ---------------------------------------------------------------------
// Waiting
// ---------------------------------------------------------------------

/// Wait until any of `objects[i]` has any of `masks[i]` or `deadline`
/// passes. Returns the observed signals of every object.
fn wait_on(objects: &[KObject], masks: &[u32], deadline: u64) -> (R<()>, Vec<u32>) {
    let pid = match current_pid() {
        Ok(p) => p,
        Err(e) => return (Err(e), Vec::new()),
    };
    loop {
        let observed: Vec<u32> = objects.iter().map(|o| o.signals()).collect();
        if observed.iter().zip(masks).any(|(s, m)| s & m != 0) {
            return (Ok(()), observed);
        }
        if deadline != kobj::INFINITE && kobj::now_ns() >= deadline {
            return (Err(RxStatus::ERR_TIMED_OUT), observed);
        }
        // Wake for the earliest of our deadline and any object's own
        // (timers)
        let mut wake_at = deadline;
        for o in objects {
            if let Some(d) = o.next_deadline() {
                wake_at = wake_at.min(d);
            }
        }
        for o in objects {
            o.signal_state().add_waiter(pid);
        }
        run::block_current(wake_at);
        for o in objects {
            o.signal_state().remove_waiter(pid);
        }
    }
}

/// object_wait_one(handle, signals, deadline_ns, *observed) — deadline is
/// absolute monotonic time (clock_get), u64::MAX = forever, 0 = poll
pub fn sys_object_wait_one(args: SyscallArgs) -> SyscallRet {
    let (h, mask, deadline, observed_ptr) = (args.arg_u32(0), args.arg_u32(1), args.arg(2) as u64, args.arg(3));
    ret((|| {
        if observed_ptr != 0 {
            user::check(observed_ptr, 4, true)?;
        }
        let (obj, _) = lookup(h, Rights::WAIT)?;
        let (r, observed) = wait_on(core::slice::from_ref(&obj), &[mask], deadline);
        if observed_ptr != 0 {
            user::write_u32(observed_ptr, observed.first().copied().unwrap_or(0))?;
        }
        r.map(|_| 0)
    })())
}

/// object_wait_many(*items, count, deadline_ns) — items are
/// `{ u32 handle; u32 waitfor; u32 pending; }`; `pending` is filled in
pub fn sys_object_wait_many(args: SyscallArgs) -> SyscallRet {
    let (items, count, deadline) = (args.arg(0), args.arg(1), args.arg(2) as u64);
    ret((|| {
        if count == 0 || count > 64 {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        user::check(items, count * 12, true)?;
        let mut objects = Vec::with_capacity(count);
        let mut masks = Vec::with_capacity(count);
        for i in 0..count {
            let h = user::read_u32(items + i * 12)?;
            masks.push(user::read_u32(items + i * 12 + 4)?);
            objects.push(lookup(h, Rights::WAIT)?.0);
        }
        let (r, observed) = wait_on(&objects, &masks, deadline);
        for (i, s) in observed.iter().enumerate() {
            user::write_u32(items + i * 12 + 8, *s)?;
        }
        r.map(|_| 0)
    })())
}

// ---------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------

/// timer_create(options, clock_id, *out)
pub fn sys_timer_create(args: SyscallArgs) -> SyscallRet {
    let out = args.arg(2);
    ret((|| {
        user::check(out, 4, true)?;
        let v = install(KObject::Timer(Timer::new()), Rights::TIMER)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

fn timer_of(h: u32) -> R<alloc::sync::Arc<Timer>> {
    // Type is checked before rights, as in Zircon
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Timer(t) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(Rights::WRITE) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(t)
}

/// timer_set(handle, deadline_ns, slack_ns)
pub fn sys_timer_set(args: SyscallArgs) -> SyscallRet {
    let (h, deadline) = (args.arg_u32(0), args.arg(1) as u64);
    ret(timer_of(h).map(|t| {
        t.set(deadline);
        0
    }))
}

/// timer_cancel(handle)
pub fn sys_timer_cancel(args: SyscallArgs) -> SyscallRet {
    ret(timer_of(args.arg_u32(0)).map(|t| {
        t.cancel();
        0
    }))
}

/// Object type of a handle (for tests and debugging): object_get_type(h)
pub fn sys_object_get_type(args: SyscallArgs) -> SyscallRet {
    ret(lookup(args.arg_u32(0), 0).map(|(o, _)| o.obj_type() as usize))
}
