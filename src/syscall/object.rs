// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Object syscalls: handles, events, event pairs, signals, waiting, timers.
//! Signatures are in docs/SYSCALL.md.

use super::{err_to_ret, ok_to_ret, user, SyscallArgs, SyscallRet};
use crate::arch::amd64::mm::RxStatus;
use crate::kobj::{self, channel, signals, Channel, Event, EventPair, KObject, Rights, Timer};
use crate::process::run;
use crate::process::table::PROCESS_TABLE;
use alloc::vec::Vec;

pub(super) type R<T> = Result<T, RxStatus>;

pub(super) fn ret(r: R<usize>) -> SyscallRet {
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

/// Give a new process a handle to its root VMAR, passed to it in RSI
/// (RDI is 0 until processes get a bootstrap channel)
pub fn give_root_vmar(p: &mut crate::process::table::Process) {
    let vmar = kobj::Vmar::root(p.aspace.clone());
    if let Some(h) = p.handles.add(KObject::Vmar(vmar), Rights(Rights::VMAR)) {
        unsafe { run::set_start_args(p.kernel_rsp, 0, h as u64) };
    }
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

/// Install both ends of a pair and write the two handle values out
fn install_pair(a: KObject, b: KObject, rights: u32, out0: usize, out1: usize) -> R<usize> {
    user::check(out0, 4, true)?;
    user::check(out1, 4, true)?;
    let h0 = install(a, rights)?;
    let h1 = match install(b, rights) {
        Ok(h) => h,
        Err(e) => {
            let _ = take(h0);
            return Err(e);
        }
    };
    user::write_u32(out0, h0)?;
    user::write_u32(out1, h1)?;
    Ok(0)
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
    let (opts, out) = (args.arg(0), args.arg(1));
    ret((|| {
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Event(Event::new()), Rights::EVENT)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// eventpair_create(options, *out0, *out1)
pub fn sys_eventpair_create(args: SyscallArgs) -> SyscallRet {
    let (opts, out0, out1) = (args.arg(0), args.arg(1), args.arg(2));
    ret((|| {
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let (a, b) = EventPair::new_pair();
        install_pair(KObject::EventPair(a), KObject::EventPair(b), Rights::EVENTPAIR, out0, out1)
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
        let (peer, allowed) = match &obj {
            KObject::EventPair(e) => (e.peer().map(KObject::EventPair), signals::USER_ALL | signals::SIGNALED),
            KObject::Channel(c) => (c.peer().map(KObject::Channel), signals::USER_ALL),
            _ => return Err(RxStatus::ERR_WRONG_TYPE),
        };
        if !rights.has(Rights::SIGNAL_PEER) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if (clear | set) & !allowed != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let peer = peer.ok_or(RxStatus::ERR_PEER_CLOSED)?;
        peer.signal_state().update(clear, set);
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
    let (opts, clock, out) = (args.arg(0), args.arg(1), args.arg(2));
    ret((|| {
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        if clock != 0 {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
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

// ---------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------

/// channel_create(options, *out0, *out1)
pub fn sys_channel_create(args: SyscallArgs) -> SyscallRet {
    let (opts, out0, out1) = (args.arg(0), args.arg(1), args.arg(2));
    ret((|| {
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let (a, b) = Channel::new_pair();
        install_pair(KObject::Channel(a), KObject::Channel(b), Rights::CHANNEL, out0, out1)
    })())
}

fn channel_of(h: u32, right: u32) -> R<alloc::sync::Arc<Channel>> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Channel(c) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(c)
}

/// channel_write(handle, options, *bytes, num_bytes, *handles, num_handles)
///
/// As in Zircon, the handles listed are always consumed: they leave the
/// caller's table even when the write fails (and are then closed).
pub fn sys_channel_write(args: SyscallArgs) -> SyscallRet {
    let (h, opts, bytes, nbytes) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    let (hptr, nh) = (args.arg(4), args.arg(5));
    ret((|| {
        if nh > channel::MAX_MSG_HANDLES {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        let raw = user::read_bytes(hptr, nh * 4)?;
        let values: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let chan = channel_of(h, Rights::WRITE);

        // Take every listed handle out of the table
        let (taken, all_found) = {
            let mut table = PROCESS_TABLE.lock();
            let cur = table.current_pid().ok_or(RxStatus::ERR_BAD_STATE)?;
            let p = table.get_mut(cur).ok_or(RxStatus::ERR_BAD_STATE)?;
            let mut taken = Vec::with_capacity(values.len());
            let mut all = true;
            for &v in &values {
                match p.handles.remove(v) {
                    Some(e) => taken.push(e),
                    None => all = false,
                }
            }
            (taken, all)
        };
        // From here on `taken` is dropped (handles closed) on any error

        let chan = chan?;
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        if !all_found {
            return Err(RxStatus::ERR_BAD_HANDLE);
        }
        if nbytes > channel::MAX_MSG_BYTES {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        for (obj, rights) in &taken {
            if !rights.has(Rights::TRANSFER) {
                return Err(RxStatus::ERR_ACCESS_DENIED);
            }
            if let KObject::Channel(c) = obj {
                if chan.is_self_or_peer(c) {
                    return Err(RxStatus::ERR_NOT_SUPPORTED);
                }
            }
        }
        let data = user::read_bytes(bytes, nbytes)?;
        chan.write(channel::Message { data, handles: taken }).map_err(|(e, _msg)| e)?;
        Ok(0)
    })())
}

/// channel_read(handle, options, *bytes, *handles, capacity, *actual)
///
/// `capacity` = num_bytes | (num_handles << 32). `actual` (may be NULL)
/// points at two u32s that receive the message's byte and handle counts,
/// also when the buffers are too small (the message then stays queued).
pub fn sys_channel_read(args: SyscallArgs) -> SyscallRet {
    let (h, opts, bytes, hptr) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    let cap = args.arg(4) as u64;
    let (cap_bytes, cap_handles) = ((cap & 0xFFFF_FFFF) as usize, (cap >> 32) as usize);
    let actual = args.arg(5);
    ret((|| {
        let chan = channel_of(h, Rights::READ)?;
        if opts != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        if actual != 0 {
            user::check(actual, 8, true)?;
        }
        let Some((nb, nh)) = chan.peek_size() else {
            return Err(if chan.peer().is_some() { RxStatus::ERR_SHOULD_WAIT } else { RxStatus::ERR_PEER_CLOSED });
        };
        if actual != 0 {
            user::write_u32(actual, nb as u32)?;
            user::write_u32(actual + 4, nh as u32)?;
        }
        if nb > cap_bytes || nh > cap_handles {
            return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
        }
        user::check(bytes, nb, true)?;
        user::check(hptr, nh * 4, true)?;
        let msg = chan.take().ok_or(RxStatus::ERR_SHOULD_WAIT)?;

        // Move the handles into this process
        let values = {
            let mut table = PROCESS_TABLE.lock();
            let cur = table.current_pid().ok_or(RxStatus::ERR_BAD_STATE)?;
            let p = table.get_mut(cur).ok_or(RxStatus::ERR_BAD_STATE)?;
            if p.handles.count() + msg.handles.len() > kobj::handle::MAX_HANDLES {
                drop(table);
                chan.untake(msg);
                return Err(RxStatus::ERR_NO_MEMORY);
            }
            let mut values = Vec::with_capacity(msg.handles.len());
            for (obj, rights) in msg.handles {
                values.push(p.handles.add(obj, rights).ok_or(RxStatus::ERR_NO_MEMORY)?);
            }
            values
        };
        user::write_bytes(bytes, &msg.data)?;
        for (i, v) in values.iter().enumerate() {
            user::write_u32(hptr + i * 4, *v)?;
        }
        Ok(0)
    })())
}
