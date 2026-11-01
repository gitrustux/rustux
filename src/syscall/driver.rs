// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Driver syscalls: interrupt objects and I/O port access. Both need the
//! root resource handle, which only `init` gets from the kernel.

use super::object::{current_process, install, lookup, ret, R};
use super::{user, SyscallArgs, SyscallRet};
use crate::arch::amd64::descriptor::{self, IO_MAP_BYTES};
use crate::arch::amd64::mm::RxStatus;
use crate::kobj::interrupt::Interrupt;
use crate::kobj::{KObject, Rights};
use alloc::sync::Arc;

/// Check that `h` is a resource handle with WRITE
fn require_resource(h: u32) -> R<()> {
    let (obj, rights) = lookup(h, 0)?;
    if !matches!(obj, KObject::Resource(_)) {
        return Err(RxStatus::ERR_WRONG_TYPE);
    }
    if !rights.has(Rights::WRITE) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(())
}

fn interrupt_of(h: u32, right: u32) -> R<Arc<Interrupt>> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Interrupt(i) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(i)
}

/// interrupt_create(resource, irq, options, *out) — bind a legacy ISA IRQ
/// (only 1, the PS/2 keyboard, for now)
pub fn sys_interrupt_create(args: SyscallArgs) -> SyscallRet {
    let (res, irq, options, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    ret((|| {
        require_resource(res)?;
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Interrupt(Interrupt::bind(irq)?), Rights::INTERRUPT)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// interrupt_wait(handle, *timestamp) — block until the IRQ fires
pub fn sys_interrupt_wait(args: SyscallArgs) -> SyscallRet {
    let (h, out) = (args.arg_u32(0), args.arg(1));
    ret((|| {
        let i = interrupt_of(h, Rights::WAIT)?;
        if out != 0 {
            user::check(out, 8, true)?;
        }
        let ts = i.wait()?;
        if out != 0 {
            user::write_u64(out, ts)?;
        }
        Ok(0)
    })())
}

/// interrupt_ack(handle)
pub fn sys_interrupt_ack(args: SyscallArgs) -> SyscallRet {
    ret(interrupt_of(args.arg_u32(0), Rights::WRITE).and_then(|i| i.ack()).map(|_| 0))
}

/// interrupt_destroy(handle) — a thread in interrupt_wait returns
/// ERR_CANCELED; the IRQ is unbound once the last handle is closed
pub fn sys_interrupt_destroy(args: SyscallArgs) -> SyscallRet {
    ret(interrupt_of(args.arg_u32(0), Rights::WRITE).map(|i| {
        i.destroy();
        0
    }))
}

fn set_ports(res: u32, port: u64, len: u64, allow: bool) -> R<usize> {
    require_resource(res)?;
    if len == 0 || port.checked_add(len).map_or(true, |e| e > 65536) {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    let p = current_process()?;
    let mut m = p.io_map.lock();
    let map = m.get_or_insert_with(|| alloc::boxed::Box::new([0xFF; IO_MAP_BYTES]));
    for port in port..port + len {
        let (byte, bit) = ((port / 8) as usize, port % 8);
        if allow {
            map[byte] &= !(1 << bit);
        } else {
            map[byte] |= 1 << bit;
        }
    }
    // The caller is running: reload its bitmap now
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem));
        descriptor::load_io_map(p.koid, m.as_deref(), true);
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
    Ok(0)
}

/// ioports_request(resource, port, len) — let the calling process use
/// `in`/`out` on ports [port, port+len)
pub fn sys_ioports_request(args: SyscallArgs) -> SyscallRet {
    ret(set_ports(args.arg_u32(0), args.arg_u64(1) & 0xFFFF, args.arg_u64(2), true))
}

/// ioports_release(resource, port, len)
pub fn sys_ioports_release(args: SyscallArgs) -> SyscallRet {
    ret(set_ports(args.arg_u32(0), args.arg_u64(1) & 0xFFFF, args.arg_u64(2), false))
}
