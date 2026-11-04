// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Creating processes and starting threads.
//!
//! [`new_process`] and [`start_thread`] back the process_create,
//! process_start and thread_start syscalls. [`spawn_elf`] is the kernel's
//! own launcher (for `init` and the legacy `spawn` call): it loads an ELF
//! from the ramdisk into a new process and starts it with a bootstrap
//! channel, like Fuchsia's processargs.

use crate::arch::amd64::mm::{kernel_map, RxStatus};
use crate::kobj::task::{Job, Process, Thread};
use crate::kobj::vmar::{opts, ROOT_BASE, ROOT_END};
use crate::kobj::{Channel, KObject, Rights, Vmar, Vmo};
use crate::mm::pmm;
use crate::process::run;
use crate::process::table::{self, PROCESS_TABLE};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Top of the stack the kernel launcher gives a program
pub const STACK_TOP: u64 = ROOT_END;
/// Size of that stack
pub const STACK_SIZE: u64 = 64 * 1024;

/// Bootstrap message handle tags (Fuchsia PA_* values). A handle's info
/// word is the type in bits 0-7 and an argument in bits 16-31 (`hnd`).
pub mod pa {
    pub const PROC_SELF: u32 = 0x01;
    pub const THREAD_SELF: u32 = 0x02;
    pub const JOB_DEFAULT: u32 = 0x03;
    pub const VMAR_ROOT: u32 = 0x04;
    /// A file descriptor; the argument is the fd number
    pub const FD: u32 = 0x30;
    /// The root resource (Fuchsia PA_RESOURCE; given to init, passed on
    /// to drivers)
    pub const ROOT_RESOURCE: u32 = 0x3F;
    /// Rustux: the kernel's input channel (USB keystrokes; given to init)
    pub const KERNEL_INPUT: u32 = 0xE0;
    /// First tag for handles a parent passes to a child (Fuchsia PA_USER0)
    pub const USER0: u32 = 0xF0;

    /// Info word for handle type `t` with argument `arg`
    pub const fn hnd(t: u32, arg: u32) -> u32 {
        (t & 0xFF) | ((arg & 0xFFFF) << 16)
    }
}

/// Bootstrap message header (Fuchsia's `zx_proc_args_t`)
pub mod procargs {
    pub const PROTOCOL: u32 = 0x4150_585d;
    pub const VERSION: u32 = 0x0001_0000;
    /// Header size in bytes: 9 u32 fields
    pub const HEADER_SIZE: usize = 36;
    /// Most bytes of arguments and environment together
    pub const MAX_STRINGS: usize = 32 * 1024;
}

/// Arguments and environment for a new program: NUL-terminated strings
/// back to back, and how many there are
#[derive(Default)]
pub struct ProcArgs {
    pub args: Vec<u8>,
    pub argc: u32,
    pub env: Vec<u8>,
    pub envc: u32,
}

impl ProcArgs {
    /// Check that `bytes` holds exactly `count` NUL-terminated strings
    pub fn valid(bytes: &[u8], count: u32) -> bool {
        let nuls = bytes.iter().filter(|&&b| b == 0).count();
        nuls == count as usize && bytes.last().map_or(true, |&b| b == 0)
    }
}

fn alloc_table_page() -> Option<u64> {
    pmm::pmm_alloc_kernel_page().ok()
}

/// A new process with an empty address space, in `job`
pub fn new_process(job: &Arc<Job>, name: &str) -> Result<Arc<Process>, RxStatus> {
    let pml4 = unsafe { kernel_map::new_user_pml4(alloc_table_page) }.map_err(|_| RxStatus::ERR_NO_MEMORY)?;
    match Process::new(job, pml4, name) {
        Ok(p) => Ok(p),
        Err(e) => {
            unsafe { kernel_map::free_user_tables(pml4, |p| { pmm::pmm_free_page(p); }) };
            Err(e)
        }
    }
}

/// Free the kernel stacks of exited threads and drop their entries
fn reap() {
    let dead = PROCESS_TABLE.lock().reap_zombies();
    for entry in dead {
        let base = entry.kernel_stack - (run::KERNEL_STACK_PAGES as u64 * 4096);
        pmm::pmm_free_contiguous(kernel_map::virt_to_phys(base), run::KERNEL_STACK_PAGES);
        drop(entry);
    }
}

/// Make `thread` runnable at `entry` with stack pointer `sp`; it finds
/// `arg1` in RDI and `arg2` in RSI. `first` is set for process_start (the
/// process must not have started), clear for thread_start (it must be
/// running). Returns the scheduler id.
pub fn start_thread(thread: &Arc<Thread>, first: bool, entry: u64, sp: u64, arg1: u64, arg2: u64) -> Result<u32, RxStatus> {
    let process = &thread.process;
    process.can_add_thread(first)?;
    if thread.tid().is_some() {
        return Err(RxStatus::ERR_BAD_STATE);
    }
    reap();
    let kstack = run::alloc_kernel_stack().map_err(|_| RxStatus::ERR_NO_MEMORY)?;
    let free_stack = || {
        let base = kstack - (run::KERNEL_STACK_PAGES as u64 * 4096);
        pmm::pmm_free_contiguous(kernel_map::virt_to_phys(base), run::KERNEL_STACK_PAGES);
    };
    let mut table = PROCESS_TABLE.lock();
    let Some(tid) = table.alloc_pid() else {
        drop(table);
        free_stack();
        return Err(RxStatus::ERR_NO_MEMORY);
    };
    if let Err(e) = process.add_thread(tid, first).and_then(|_| thread.set_running(tid)) {
        drop(table);
        free_stack();
        return Err(e);
    }
    let ppid = table.current_pid().unwrap_or(0);
    let mut e = table::Process::new(tid, ppid, thread.clone(), kstack, sp, entry);
    unsafe { run::set_start_args(e.kernel_rsp, arg1, arg2) };
    e.set_name(process.name.clone());
    run::reset_flags(tid);
    table.insert(e);
    Ok(tid)
}

fn rd16(b: &[u8], o: usize) -> Option<u64> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as u64)
}
fn rd32(b: &[u8], o: usize) -> Option<u64> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as u64)
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Load a static x86-64 ELF executable into `process`'s address space and
/// return its entry point. All PT_LOAD segments go into one VMO; each page
/// is mapped with the union of the permissions of the segments covering it.
pub fn load_elf(process: &Process, elf: &[u8]) -> Result<u64, &'static str> {
    const BAD: &str = "not a static x86-64 ELF executable";
    if elf.get(0..4) != Some(b"\x7fELF") || elf.get(4) != Some(&2) || rd16(elf, 18) != Some(0x3E) {
        return Err(BAD);
    }
    let entry = rd64(elf, 24).ok_or(BAD)?;
    let phoff = rd64(elf, 32).ok_or(BAD)? as usize;
    let phentsize = rd16(elf, 54).ok_or(BAD)? as usize;
    let phnum = rd16(elf, 56).ok_or(BAD)? as usize;

    struct Seg { vaddr: u64, off: usize, filesz: u64, memsz: u64, flags: u64 }
    let mut segs = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if rd32(elf, ph).ok_or(BAD)? != 1 {
            continue; // not PT_LOAD
        }
        let s = Seg {
            flags: rd32(elf, ph + 4).ok_or(BAD)?,
            off: rd64(elf, ph + 8).ok_or(BAD)? as usize,
            vaddr: rd64(elf, ph + 16).ok_or(BAD)?,
            filesz: rd64(elf, ph + 32).ok_or(BAD)?,
            memsz: rd64(elf, ph + 40).ok_or(BAD)?,
        };
        if s.filesz > s.memsz || s.off.checked_add(s.filesz as usize).map_or(true, |e| e > elf.len()) {
            return Err(BAD);
        }
        if s.memsz > 0 {
            segs.push(s);
        }
    }
    let lo = segs.iter().map(|s| s.vaddr & !0xFFF).min().ok_or(BAD)?;
    let hi = segs.iter().map(|s| (s.vaddr + s.memsz + 0xFFF) & !0xFFF).max().ok_or(BAD)?;
    if lo < ROOT_BASE || hi > STACK_TOP - STACK_SIZE || hi - lo > 64 << 20 {
        return Err("ELF image outside the user address range");
    }
    let vmo = Vmo::new(hi - lo).map_err(|_| "out of memory (image)")?;
    let mut perms = alloc::vec![0u32; ((hi - lo) / 4096) as usize];
    for s in &segs {
        unsafe { vmo.write_from(s.vaddr - lo, elf.as_ptr().add(s.off), s.filesz) }.map_err(|_| "out of memory (image)")?;
        let mut p = opts::PERM_READ; // x86 pages are always readable
        if s.flags & 2 != 0 {
            p |= opts::PERM_WRITE;
        }
        if s.flags & 1 != 0 {
            p |= opts::PERM_EXECUTE;
        }
        let first = ((s.vaddr & !0xFFF) - lo) / 4096;
        let last = ((s.vaddr + s.memsz + 0xFFF) & !0xFFF) - lo;
        for page in first..last / 4096 {
            perms[page as usize] |= p;
        }
    }
    // Map runs of pages with equal permissions
    let mut start = 0usize;
    while start < perms.len() {
        let mut end = start + 1;
        while end < perms.len() && perms[end] == perms[start] {
            end += 1;
        }
        if perms[start] != 0 {
            let addr = lo + start as u64 * 4096;
            process.aspace
                .map(ROOT_BASE, ROOT_END, Some(addr), vmo.clone(), start as u64 * 4096,
                     (end - start) as u64 * 4096, perms[start], perms[start])
                .map_err(|_| "cannot map ELF image")?;
        }
        start = end;
    }
    Ok(entry)
}

/// Map a fresh stack below [`STACK_TOP`]; returns the initial stack
/// pointer. As in Fuchsia, it is 8 below a 16-byte boundary, as if the
/// entry point had been called: compilers treat `_start` as an ordinary
/// function and align SSE spills on that assumption.
pub fn map_stack(process: &Process) -> Result<u64, RxStatus> {
    let vmo = Vmo::new(STACK_SIZE)?;
    let rw = opts::PERM_READ | opts::PERM_WRITE;
    process.aspace.map(ROOT_BASE, ROOT_END, Some(STACK_TOP - STACK_SIZE), vmo, 0, STACK_SIZE, rw, rw)?;
    Ok(STACK_TOP - 8)
}

/// Load `/bin/...` from the ramdisk and start it as a new process in
/// `job`. The program gets a bootstrap channel in RDI holding one message
/// in Fuchsia's processargs format (`docs/SYSCALL.md`): a header, an info
/// word per handle (see [`pa`]), then the arguments and the environment.
/// The handles are to itself, its first thread, its job and its root
/// VMAR, followed by `extra`. With no arguments, argv is just `path`.
pub fn spawn_elf(path: &str, job: &Arc<Job>, extra: Vec<(u32, KObject, Rights)>, mut args: ProcArgs) -> Result<(Arc<Process>, u32), RxStatus> {
    let ramdisk = crate::fs::ramdisk::get_ramdisk().map_err(|_| RxStatus::ERR_NOT_FOUND)?;
    let file = ramdisk.find_file(path).ok_or(RxStatus::ERR_NOT_FOUND)?;
    let elf = unsafe {
        core::slice::from_raw_parts(ramdisk.data.as_ptr().add(file.data_offset as usize), file.size as usize)
    };
    let name = path.rsplit('/').next().unwrap_or(path);
    let process = new_process(job, name)?;
    let entry = load_elf(&process, elf).map_err(|e| {
        crate::kobj::debugcon("[SPAWN] ");
        crate::kobj::debugcon(e);
        crate::kobj::debugcon("\n");
        RxStatus::ERR_INVALID_ARGS
    })?;
    let sp = map_stack(&process)?;
    let thread = Thread::new(&process)?;

    let (ours, theirs) = Channel::new_pair();
    let mut tags = alloc::vec![pa::PROC_SELF, pa::THREAD_SELF, pa::JOB_DEFAULT, pa::VMAR_ROOT];
    let mut handles = alloc::vec![
        (KObject::Process(process.clone()), Rights(Rights::PROCESS)),
        (KObject::Thread(thread.clone()), Rights(Rights::THREAD)),
        (KObject::Job(job.clone()), Rights(Rights::JOB)),
        (KObject::Vmar(Vmar::root(process.aspace.clone())), Rights(Rights::VMAR)),
    ];
    for (tag, obj, rights) in extra {
        tags.push(tag);
        handles.push((obj, rights));
    }
    if args.argc == 0 {
        args.args = path.as_bytes().to_vec();
        args.args.push(0);
        args.argc = 1;
    }
    let info_off = procargs::HEADER_SIZE;
    let args_off = info_off + tags.len() * 4;
    let env_off = args_off + args.args.len();
    let header = [
        procargs::PROTOCOL, procargs::VERSION, info_off as u32,
        args_off as u32, args.argc, env_off as u32, args.envc, 0, 0,
    ];
    let mut data: Vec<u8> = header.iter().chain(tags.iter()).flat_map(|t| t.to_le_bytes()).collect();
    data.extend_from_slice(&args.args);
    data.extend_from_slice(&args.env);
    ours.write(crate::kobj::channel::Message { data, handles }).map_err(|(e, _)| e)?;
    drop(ours);
    let boot = process.handles.lock()
        .add(KObject::Channel(theirs), Rights(Rights::CHANNEL))
        .ok_or(RxStatus::ERR_NO_MEMORY)?;
    let tid = start_thread(&thread, true, entry, sp, boot as u64, 0)?;
    Ok((process, tid))
}
