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

/// Check that `h` is the root resource (or, with `also`, a resource of
/// that kind) with WRITE
pub fn require_resource_kind(h: u32, also: Option<u32>) -> R<()> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Resource(r) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(Rights::WRITE) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    if r.kind != crate::kobj::interrupt::kind::ROOT && Some(r.kind) != also {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(())
}

/// Check that `h` is the root resource with WRITE
pub fn require_resource(h: u32) -> R<()> {
    require_resource_kind(h, None)
}

/// Check that `h` allows ports [port, port+len): the root resource, or an
/// IOPORT resource covering them
fn require_ports(h: u32, port: u64, len: u64) -> R<()> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Resource(r) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(Rights::WRITE) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    match r.kind {
        crate::kobj::interrupt::kind::ROOT => Ok(()),
        crate::kobj::interrupt::kind::IOPORT if port >= r.base && port + len <= r.base + r.size => Ok(()),
        _ => Err(RxStatus::ERR_ACCESS_DENIED),
    }
}

fn interrupt_of(h: u32, right: u32) -> R<Arc<Interrupt>> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Interrupt(i) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(i)
}

/// Option of debuglog_create: a read-only handle for the kernel log
/// (needs no resource), as Zircon's ZX_LOG_FLAG_READABLE
pub const LOG_FLAG_READABLE: usize = 0x4000_0000;

/// debuglog_create(resource, options, *out) — a debug log handle: for
/// writing (needs the root resource) or, with LOG_FLAG_READABLE, for
/// reading the kernel log (anyone)
pub fn sys_debuglog_create(args: SyscallArgs) -> SyscallRet {
    let (res, options, out) = (args.arg_u32(0), args.arg(1), args.arg(2));
    ret((|| {
        let rights = match options {
            0 => {
                require_resource(res)?;
                Rights::LOG
            }
            LOG_FLAG_READABLE => Rights::BASIC | Rights::READ,
            _ => return Err(RxStatus::ERR_INVALID_ARGS),
        };
        user::check(out, 4, true)?;
        let v = install(KObject::DebugLog(crate::kobj::debuglog::DebugLog::new()), rights)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// debuglog_write(handle, options, *buf, len) — print `len` bytes (at most
/// 4096) on the kernel console
pub fn sys_debuglog_write(args: SyscallArgs) -> SyscallRet {
    let (h, options, buf, len) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    ret((|| {
        let (obj, rights) = lookup(h, 0)?;
        let KObject::DebugLog(log) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        if !rights.has(Rights::WRITE) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if options != 0 || len > crate::kobj::debuglog::MAX_WRITE {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let bytes = user::read_bytes(buf, len)?;
        log.write(&bytes);
        Ok(0)
    })())
}

/// debuglog_read(handle, options, *buf, len) — kernel log bytes not read
/// yet through this handle (from the oldest kept, 64 KiB); SHOULD_WAIT when
/// there are none. Returns the count.
pub fn sys_debuglog_read(args: SyscallArgs) -> SyscallRet {
    let (h, options, buf, len) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    ret((|| {
        let (obj, rights) = lookup(h, 0)?;
        let KObject::DebugLog(log) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        if !rights.has(Rights::READ) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(buf, len, true)?;
        let mut tmp = alloc::vec![0u8; len.min(crate::klog::RING)];
        let n = log.read(&mut tmp);
        if n == 0 {
            return Err(RxStatus::ERR_SHOULD_WAIT);
        }
        user::write_bytes(buf, &tmp[..n])?;
        Ok(n)
    })())
}

/// resource_create(parent, kind, *out, base, size) — a narrower resource
/// from the root resource: POWER (1), or IOPORT (2) for ports
/// [base, base+size)
pub fn sys_resource_create(args: SyscallArgs) -> SyscallRet {
    let (parent, kind, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2));
    let (base, size) = (args.arg_u64(3), args.arg_u64(4));
    ret((|| {
        use crate::kobj::interrupt::{kind as k, Resource};
        require_resource(parent)?;
        let r = match kind {
            k::POWER => Resource::new(kind),
            k::IOPORT => {
                if size == 0 || base.checked_add(size).map_or(true, |e| e > 65536) {
                    return Err(RxStatus::ERR_INVALID_ARGS);
                }
                Resource::new_range(kind, base, size)
            }
            _ => return Err(RxStatus::ERR_INVALID_ARGS),
        };
        user::check(out, 4, true)?;
        let v = install(KObject::Resource(r), Rights::RESOURCE)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// interrupt_create option: an MSI vector instead of an ISA IRQ
pub const INTERRUPT_MSI: usize = 0x100;

/// interrupt_create(resource, irq, options, *out) — bind a legacy ISA IRQ
/// (only 1, the PS/2 keyboard, for now), or with INTERRUPT_MSI (irq 0) a
/// free MSI vector; object_get_info(INTERRUPT_MSI) then tells the address
/// and data to program into the device
pub fn sys_interrupt_create(args: SyscallArgs) -> SyscallRet {
    let (res, irq, options, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3));
    ret((|| {
        require_resource(res)?;
        let i = match options {
            0 => Interrupt::bind(irq)?,
            INTERRUPT_MSI if irq == 0 => Interrupt::bind_msi()?,
            _ => return Err(RxStatus::ERR_INVALID_ARGS),
        };
        user::check(out, 4, true)?;
        let v = install(KObject::Interrupt(i), Rights::INTERRUPT)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// interrupt_wait(handle, *timestamp) — block until the IRQ fires
pub fn sys_interrupt_wait(args: SyscallArgs) -> SyscallRet {
    let (h, out) = (args.arg_u32(0), args.arg(1));
    // deadline: monotonic ns; 0 = none (older callers pass 0)
    let deadline = match args.arg(2) as u64 { 0 => crate::kobj::INFINITE, d => d };
    ret((|| {
        let i = interrupt_of(h, Rights::WAIT)?;
        if out != 0 {
            user::check(out, 8, true)?;
        }
        let ts = i.wait(deadline)?;
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
    if len == 0 || port.checked_add(len).map_or(true, |e| e > 65536) {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    require_ports(res, port, len)?;
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

// ---------------------------------------------------------------------
// DMA: bus transaction initiators and pinned memory
// ---------------------------------------------------------------------

fn bti_of(h: u32, right: u32) -> R<Arc<crate::kobj::bti::Bti>> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Bti(b) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok(b)
}

/// bti_create(resource, options, bti_id, *out) — a BTI for one device
/// (the device manager passes bus:device.function as the id)
pub fn sys_bti_create(args: SyscallArgs) -> SyscallRet {
    let (res, options, id, out) = (args.arg_u32(0), args.arg(1), args.arg_u64(2), args.arg(3));
    ret((|| {
        require_resource(res)?;
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Bti(crate::kobj::bti::Bti::new(id)), Rights::BTI)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// bti_pin options (Zircon values)
pub const BTI_PERM_READ: u64 = 1 << 0;
pub const BTI_PERM_WRITE: u64 = 1 << 1;
pub const BTI_CONTIGUOUS: u64 = 1 << 3;

/// bti_pin(bti | options << 32, vmo, offset, size, *addrs, *pmt) — pin
/// [offset, offset+size) of a VMO for DMA and write the device address of
/// each page to `addrs` (size / 4096 entries; one entry with
/// BTI_CONTIGUOUS, which needs a contiguous VMO). The pages stay until
/// pmt_unpin or the PMT handle is closed. Physical VMOs cannot be pinned.
pub fn sys_bti_pin(args: SyscallArgs) -> SyscallRet {
    let (h, options) = ((args.arg_u64(0) & 0xFFFF_FFFF) as u32, args.arg_u64(0) >> 32);
    let (vh, offset, size, addrs, out) = (args.arg_u32(1), args.arg_u64(2), args.arg_u64(3), args.arg(4), args.arg(5));
    ret((|| {
        use crate::kobj::vmo::{VmoKind, PAGE};
        let bti = bti_of(h, Rights::MAP)?;
        if options & !(BTI_PERM_READ | BTI_PERM_WRITE | BTI_CONTIGUOUS) != 0
            || options & (BTI_PERM_READ | BTI_PERM_WRITE) == 0
            || size == 0 || offset % PAGE != 0 || size % PAGE != 0 || size > crate::kobj::vmo::MAX_SIZE
        {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let (obj, rights) = lookup(vh, Rights::MAP)?;
        let KObject::Vmo(vmo) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        if (options & BTI_PERM_READ != 0 && !rights.has(Rights::READ))
            || (options & BTI_PERM_WRITE != 0 && !rights.has(Rights::WRITE))
        {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if vmo.kind == VmoKind::Physical {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        let contiguous = options & BTI_CONTIGUOUS != 0;
        if contiguous && vmo.kind != VmoKind::Contiguous {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let n = if contiguous { 1 } else { (size / PAGE) as usize };
        user::check(addrs, n * 8, true)?;
        user::check(out, 4, true)?;
        let pages = vmo.phys_pages(offset, size)?;
        for (i, p) in pages.iter().take(n).enumerate() {
            user::write_u64(addrs + i * 8, *p)?;
        }
        let pmt = crate::kobj::bti::Pmt::new(bti, vmo, offset, size);
        let v = install(KObject::Pmt(pmt), Rights::PMT)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// pmt_unpin(pmt) — release pinned pages; the handle is closed
pub fn sys_pmt_unpin(args: SyscallArgs) -> SyscallRet {
    ret((|| {
        let (obj, _) = lookup(args.arg_u32(0), 0)?;
        let KObject::Pmt(p) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        p.unpin();
        drop(p);
        let _ = super::object::take(args.arg_u32(0))?;
        Ok(0)
    })())
}

// ---------------------------------------------------------------------
// ACPI tables
// ---------------------------------------------------------------------

/// acpi_table_read(resource, signature, index, *buf, len) — copy the
/// `index`th ACPI table with this signature (four ASCII bytes, little
/// endian: "MCFG" is 0x4746434D) into buf; returns the table's length
/// (call with len 0 to learn it). NOT_FOUND if there is none.
pub fn sys_acpi_table_read(args: SyscallArgs) -> SyscallRet {
    let (res, sig, index, buf, len) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3), args.arg(4));
    ret((|| {
        require_resource(res)?;
        let sig = sig.to_le_bytes();
        let t = unsafe { crate::arch::amd64::smp::find_table_nth(&sig, index) }.ok_or(RxStatus::ERR_NOT_FOUND)?;
        let virt = crate::arch::amd64::mm::kernel_map::phys_to_virt(t);
        let tlen = unsafe { core::ptr::read_unaligned((virt + 4) as *const u32) } as usize;
        let n = tlen.min(len);
        if n > 0 {
            user::check(buf, n, true)?;
            let bytes = unsafe { core::slice::from_raw_parts(virt as *const u8, n) };
            user::write_bytes(buf, bytes)?;
        }
        Ok(tlen)
    })())
}

// ---------------------------------------------------------------------
// USB mass storage (step 13)
// ---------------------------------------------------------------------

/// The fixed layout usb_list fills (rx_usb_info_t in <rustux/syscalls.h>)
const USB_INFO_SIZE: usize = 64;

fn usb_of(h: u32, rights: u32) -> R<u32> {
    let (obj, _) = lookup(h, rights)?;
    let KObject::Usb(u) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    Ok(u.id)
}

fn storage_status(e: crate::drivers::usb::StorageError) -> RxStatus {
    use crate::drivers::usb::StorageError::*;
    match e {
        Gone => RxStatus::ERR_PEER_CLOSED,
        Stall => RxStatus::ERR_IO_REFUSED,
        Timeout => RxStatus::ERR_TIMED_OUT,
        Io => RxStatus::ERR_IO,
        Invalid => RxStatus::ERR_INVALID_ARGS,
    }
}

/// usb_list(resource, *buf, count) — the USB mass-storage devices: up to
/// `count` rx_usb_info_t records; returns how many there are
pub fn sys_usb_list(args: SyscallArgs) -> SyscallRet {
    let (res, buf, count) = (args.arg_u32(0), args.arg(1), args.arg(2));
    ret((|| {
        require_resource(res)?;
        let list = crate::drivers::usb::storage_list();
        let n = list.len().min(count);
        if n > 0 {
            user::check(buf, n * USB_INFO_SIZE, true)?;
            let mut out = alloc::vec![0u8; n * USB_INFO_SIZE];
            for (k, s) in list.iter().take(n).enumerate() {
                let o = &mut out[k * USB_INFO_SIZE..(k + 1) * USB_INFO_SIZE];
                o[0..4].copy_from_slice(&s.id.to_le_bytes());
                o[4..6].copy_from_slice(&s.vid.to_le_bytes());
                o[6..8].copy_from_slice(&s.pid.to_le_bytes());
                o[8] = 8; // class: mass storage
                o[9] = s.subclass;
                o[10] = s.protocol;
                o[11] = s.speed;
                o[12] = s.root_port;
                o[13] = s.interface;
                o[16..64].copy_from_slice(&s.product);
            }
            user::write_bytes(buf, &out)?;
        }
        Ok(list.len())
    })())
}

/// usb_open(resource, id, *out) — a handle to USB device `id`
pub fn sys_usb_open(args: SyscallArgs) -> SyscallRet {
    let (res, id, out) = (args.arg_u32(0), args.arg_u32(1), args.arg(2));
    ret((|| {
        require_resource(res)?;
        user::check(out, 4, true)?;
        if !crate::drivers::usb::storage_exists(id) {
            return Err(RxStatus::ERR_NOT_FOUND);
        }
        let h = install(KObject::Usb(crate::kobj::usb::UsbDevice::new(id)), Rights::USB)?;
        user::write_u32(out, h)?;
        Ok(0)
    })())
}

/// usb_bulk(handle, options, *buf, len, timeout_ms) — one bulk transfer:
/// options bit 0 set = IN (device to buf), clear = OUT. At most 64 KiB.
/// Returns the bytes moved; IO_REFUSED when the endpoint stalled (clear
/// it with usb_clear_halt), PEER_CLOSED when the device is gone.
pub fn sys_usb_bulk(args: SyscallArgs) -> SyscallRet {
    let (h, opts, buf, len, ms) = (args.arg_u32(0), args.arg_u32(1), args.arg(2), args.arg(3), args.arg(4) as u64);
    ret((|| {
        let input = opts & 1 != 0;
        let id = usb_of(h, if input { Rights::READ } else { Rights::WRITE })?;
        if len > crate::drivers::usb::BULK_MAX {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        let ms = if ms == 0 || ms > 30000 { 5000 } else { ms };
        if input {
            user::check(buf, len, true)?;
            let mut data = alloc::vec![0u8; len];
            let n = crate::drivers::usb::storage_bulk_in(id, &mut data, ms).map_err(storage_status)?;
            user::write_bytes(buf, &data[..n])?;
            Ok(n)
        } else {
            let data = if len > 0 { user::read_bytes(buf, len)? } else { alloc::vec::Vec::new() };
            crate::drivers::usb::storage_bulk_out(id, &data, ms).map_err(storage_status)
        }
    })())
}

/// usb_control(handle, *setup8, *buf, len) — a control request for the
/// device's storage interface (class requests such as Get Max LUN and
/// Bulk-Only Reset, GET_DESCRIPTOR); IN data or none. Returns the bytes
/// received.
pub fn sys_usb_control(args: SyscallArgs) -> SyscallRet {
    let (h, setup, buf, len) = (args.arg_u32(0), args.arg(1), args.arg(2), args.arg(3));
    ret((|| {
        let id = usb_of(h, Rights::WRITE)?;
        let s = user::read_bytes(setup, 8)?;
        let mut st = [0u8; 8];
        st.copy_from_slice(&s);
        let len = len.min(4096);
        if len > 0 {
            user::check(buf, len, true)?;
        }
        let mut data = alloc::vec![0u8; len];
        let n = crate::drivers::usb::storage_control(id, st, &mut data).map_err(storage_status)?;
        if n > 0 {
            user::write_bytes(buf, &data[..n])?;
        }
        Ok(n)
    })())
}

/// usb_clear_halt(handle, options) — recover a stalled bulk endpoint
/// (bit 0: the IN endpoint, else OUT)
pub fn sys_usb_clear_halt(args: SyscallArgs) -> SyscallRet {
    let (h, opts) = (args.arg_u32(0), args.arg_u32(1));
    ret((|| {
        let id = usb_of(h, Rights::WRITE)?;
        crate::drivers::usb::storage_clear_halt(id, opts & 1 != 0).map_err(storage_status)?;
        Ok(0)
    })())
}
