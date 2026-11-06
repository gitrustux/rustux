// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Memory syscalls: VMOs and VMARs. Signatures are in docs/SYSCALL.md.

use super::object::{install, lookup, ret, R};
use super::{user, SyscallArgs, SyscallRet};
use crate::arch::amd64::mm::RxStatus;
use crate::kobj::vmar::opts;
use crate::kobj::{KObject, Rights, Vmar, Vmo};
use alloc::sync::Arc;

fn vmo_of(h: u32, right: u32) -> R<(Arc<Vmo>, Rights)> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Vmo(v) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    if !rights.has(right) {
        return Err(RxStatus::ERR_ACCESS_DENIED);
    }
    Ok((v, rights))
}

fn vmar_of(h: u32) -> R<(Arc<Vmar>, Rights)> {
    let (obj, rights) = lookup(h, 0)?;
    let KObject::Vmar(v) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
    Ok((v, rights))
}

/// Mapping permissions a handle's rights allow
fn perms_allowed(r: Rights) -> u32 {
    let mut p = 0;
    if r.has(Rights::READ) {
        p |= opts::PERM_READ;
    }
    if r.has(Rights::WRITE) {
        p |= opts::PERM_WRITE;
    }
    if r.has(Rights::EXECUTE) {
        p |= opts::PERM_EXECUTE;
    }
    p
}

/// vmo_create(size, options, *out)
pub fn sys_vmo_create(args: SyscallArgs) -> SyscallRet {
    let (size, options, out) = (args.arg_u64(0), args.arg(1), args.arg(2));
    ret((|| {
        if options != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Vmo(Vmo::new(size)?), Rights::VMO)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// vmo_read(handle, *buffer, offset, len)
pub fn sys_vmo_read(args: SyscallArgs) -> SyscallRet {
    let (h, buf, offset, len) = (args.arg_u32(0), args.arg(1), args.arg_u64(2), args.arg(3));
    ret((|| {
        let (vmo, _) = vmo_of(h, Rights::READ)?;
        user::check(buf, len, true)?;
        unsafe { vmo.read_to(offset, buf as *mut u8, len as u64)? };
        Ok(0)
    })())
}

/// vmo_write(handle, *buffer, offset, len)
pub fn sys_vmo_write(args: SyscallArgs) -> SyscallRet {
    let (h, buf, offset, len) = (args.arg_u32(0), args.arg(1), args.arg_u64(2), args.arg(3));
    ret((|| {
        let (vmo, _) = vmo_of(h, Rights::WRITE)?;
        user::check(buf, len, false)?;
        unsafe { vmo.write_from(offset, buf as *const u8, len as u64)? };
        Ok(0)
    })())
}

/// vmo_clone(handle, options, offset, size, *out) — like
/// zx_vmo_create_child with ZX_VMO_CHILD_SNAPSHOT: the child starts as a
/// copy of the range and the two are independent afterwards
pub fn sys_vmo_clone(args: SyscallArgs) -> SyscallRet {
    let (h, options, offset, size, out) = (args.arg_u32(0), args.arg(1), args.arg_u64(2), args.arg_u64(3), args.arg(4));
    ret((|| {
        let (vmo, _) = vmo_of(h, Rights::DUPLICATE | Rights::READ)?;
        if options & !1 != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let child = vmo.snapshot(offset, size)?;
        let v = install(KObject::Vmo(child), Rights::VMO)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// vmo_get_size(handle, *size)
pub fn sys_vmo_get_size(args: SyscallArgs) -> SyscallRet {
    let (h, out) = (args.arg_u32(0), args.arg(1));
    ret((|| {
        let (vmo, _) = vmo_of(h, 0)?;
        user::write_u64(out, vmo.size())?;
        Ok(0)
    })())
}

/// vmo_replace_as_executable(handle, vmex, *out) — the VMO handle is
/// replaced by one that also has EXECUTE (closed even on failure). `vmex`
/// is not checked yet: any process may do this.
pub fn sys_vmo_replace_as_executable(args: SyscallArgs) -> SyscallRet {
    let (h, out) = (args.arg_u32(0), args.arg(2));
    ret((|| {
        user::check(out, 4, true)?;
        let (obj, rights) = super::object::take(h)?;
        if !matches!(obj, KObject::Vmo(_)) {
            return Err(RxStatus::ERR_WRONG_TYPE);
        }
        let v = install(obj, rights.0 | Rights::EXECUTE)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// vmar_map(vmar, options | (vmo << 32), vmar_offset, vmo_offset, len, *mapped_addr)
pub fn sys_vmar_map(args: SyscallArgs) -> SyscallRet {
    let (vh, packed, vmar_offset) = (args.arg_u32(0), args.arg_u64(1), args.arg_u64(2));
    let (options, oh) = (packed as u32, (packed >> 32) as u32);
    let (vmo_offset, len, out) = (args.arg_u64(3), args.arg_u64(4), args.arg(5));
    ret((|| {
        let (vmar, vrights) = vmar_of(vh)?;
        let (vmo, orights) = vmo_of(oh, Rights::MAP)?;
        if options & !(opts::PERMS | opts::SPECIFIC) != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let perms = options & opts::PERMS;
        let max = perms_allowed(vrights) & perms_allowed(orights);
        if perms & !max != 0 {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        let addr = if options & opts::SPECIFIC != 0 {
            Some(vmar.base.checked_add(vmar_offset).ok_or(RxStatus::ERR_INVALID_ARGS)?)
        } else if vmar_offset != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        } else {
            None
        };
        user::check(out, 8, true)?;
        let base = vmar.aspace.map(vmar.base, vmar.end(), addr, vmo, vmo_offset, len, perms, max)?;
        user::write_u64(out, base)?;
        Ok(0)
    })())
}

/// vmar_unmap(vmar, addr, len)
pub fn sys_vmar_unmap(args: SyscallArgs) -> SyscallRet {
    let (vh, addr, len) = (args.arg_u32(0), args.arg_u64(1), args.arg_u64(2));
    ret((|| {
        let (vmar, _) = vmar_of(vh)?;
        if !vmar.contains(addr, len) {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let removed = vmar.aspace.unmap(addr, len)?;
        drop(removed);
        Ok(0)
    })())
}

/// vmar_protect(vmar, options, addr, len)
pub fn sys_vmar_protect(args: SyscallArgs) -> SyscallRet {
    let (vh, options, addr, len) = (args.arg_u32(0), args.arg_u32(1), args.arg_u64(2), args.arg_u64(3));
    ret((|| {
        let (vmar, vrights) = vmar_of(vh)?;
        if options & !opts::PERMS != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        if options & !perms_allowed(vrights) != 0 {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if !vmar.contains(addr, len) {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        vmar.aspace.protect(addr, len, options)?;
        Ok(0)
    })())
}

/// vmo_create_physical(resource, paddr, size, *out) — a VMO over device
/// memory (registers, frame buffers). Needs the root resource; the range
/// must be page aligned and outside RAM and firmware memory. Mapped
/// uncached (UNCACHED_DEVICE) unless the policy is changed. Cannot be read
/// or written with vmo_read/vmo_write: map it.
pub fn sys_vmo_create_physical(args: SyscallArgs) -> SyscallRet {
    let (res, paddr, size, out) = (args.arg_u32(0), args.arg_u64(1), args.arg_u64(2), args.arg(3));
    ret((|| {
        super::driver::require_resource(res)?;
        if paddr % 4096 != 0 || size == 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let end = paddr.checked_add(size).ok_or(RxStatus::ERR_OUT_OF_RANGE)?;
        let (kbase, ksize) = crate::arch::amd64::mm::kernel_map::kernel_image();
        if crate::mm::pmm::overlaps_ram(paddr, end) || (paddr < kbase + ksize && kbase < end) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        user::check(out, 4, true)?;
        let v = install(KObject::Vmo(Vmo::new_physical(paddr, size)?), Rights::VMO)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}

/// vmo_set_cache_policy(vmo, policy) — CACHED 0, UNCACHED 1,
/// UNCACHED_DEVICE 2 (WRITE_COMBINING 3 is not supported yet); applies to
/// later mappings
pub fn sys_vmo_set_cache_policy(args: SyscallArgs) -> SyscallRet {
    let (h, policy) = (args.arg_u32(0), args.arg_u32(1));
    ret((|| {
        let (vmo, _) = vmo_of(h, Rights::SET_PROPERTY)?;
        vmo.set_cache_policy(policy)?;
        Ok(0)
    })())
}

/// vmo_create_contiguous option: below 4 GiB (devices with 32-bit DMA)
pub const VMO_CONTIGUOUS_LOW: usize = 0x100;

/// vmo_create_contiguous(bti, size, options, *out) — zeroed, physically
/// contiguous memory for DMA. options: log2 of the alignment in bits 0-7
/// (0 = page aligned), VMO_CONTIGUOUS_LOW.
pub fn sys_vmo_create_contiguous(args: SyscallArgs) -> SyscallRet {
    let (bti, size, options, out) = (args.arg_u32(0), args.arg_u64(1), args.arg(2), args.arg(3));
    ret((|| {
        let (obj, rights) = lookup(bti, 0)?;
        let KObject::Bti(_) = obj else { return Err(RxStatus::ERR_WRONG_TYPE) };
        if !rights.has(Rights::MAP) {
            return Err(RxStatus::ERR_ACCESS_DENIED);
        }
        if options & !(0xFF | VMO_CONTIGUOUS_LOW) != 0 || size == 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        user::check(out, 4, true)?;
        let vmo = Vmo::new_contiguous(size, (options & 0xFF) as u8, options & VMO_CONTIGUOUS_LOW != 0)?;
        let v = install(KObject::Vmo(vmo), Rights::VMO)?;
        user::write_u32(out, v)?;
        Ok(0)
    })())
}
