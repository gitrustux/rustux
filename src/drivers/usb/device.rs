// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Device enumeration: root ports, USB 2.0 hubs (nested), HID boot
//! keyboards, mass storage (bulk-only transport, step 13: the kernel sets
//! up the two bulk endpoints and moves data; the SCSI driver is the
//! userspace `usb-storage`, which reaches the device through a USB handle),
//! and root-port hot-plug (a KVM switch disconnects its whole hub from the
//! root port).
//!
//! Each slot has its own Input Context, Device Context, EP0 ring and
//! descriptor buffer (DMA pages, reused when the slot id is reused), so a
//! hub stays configured while the devices behind it are set up.

use super::ring::{dma_page, Ring};
use super::xhci::{
    self, Controller, Setup, CC_SHORT_PACKET, CC_STALL, CC_SUCCESS, EVT_PORT_STATUS, EVT_TRANSFER, MAX_SLOTS,
    PORTSC_CCS, PORTSC_CSC, PORTSC_PED, PORTSC_SPEED_SHIFT, SPEED_FULL, SPEED_HIGH, SPEED_LOW,
    SPEED_SUPER, TRB_NORMAL,
};
use super::{log, log_dec, log_hex, Result, UsbError};
use crate::arch::amd64::tsc::delay_ms;
use crate::mm::dma::DmaBuffer;

const MAX_KEYBOARDS: usize = 4;
const MAX_STORAGE: usize = 8;
/// Largest bulk transfer (the transfer buffer: 64 KiB, 64 KiB aligned)
pub const BULK_MAX: usize = 65536;
const MAX_HUB_DEPTH: u8 = 4;
const REPORT_SIZE: usize = 8;

// Requests and descriptors
const REQ_GET_STATUS: u8 = 0x00;
const REQ_CLEAR_FEATURE: u8 = 0x01;
const REQ_SET_FEATURE: u8 = 0x03;
const REQ_GET_DESCRIPTOR: u8 = 0x06;
const REQ_SET_CONFIGURATION: u8 = 0x09;
const REQ_SET_IDLE: u8 = 0x0A;
const REQ_SET_PROTOCOL: u8 = 0x0B;
const DT_DEVICE: u8 = 1;
const DT_CONFIG: u8 = 2;
const DT_INTERFACE: u8 = 4;
const DT_ENDPOINT: u8 = 5;
const DT_HUB: u8 = 0x29;
const CLASS_HID: u8 = 3;
const CLASS_HUB: u8 = 9;
const CLASS_STORAGE: u8 = 8;
const DT_STRING: u8 = 3;
const DT_SS_EP_COMPANION: u8 = 0x30;
const FEATURE_ENDPOINT_HALT: u16 = 0;

// Hub class features and port status bits (USB 2.0 ch. 11.24)
const HUB_PORT_RESET: u16 = 4;
const HUB_PORT_POWER: u16 = 8;
const HUB_C_PORT_CONNECTION: u16 = 16;
const HUB_C_PORT_RESET: u16 = 20;
const HUB_STAT_CONNECTION: u16 = 0x0001;
const HUB_STAT_ENABLE: u16 = 0x0002;
const HUB_STAT_LOW_SPEED: u16 = 0x0200;
const HUB_STAT_HIGH_SPEED: u16 = 0x0400;
const HUB_CHANGE_RESET: u16 = 0x0010;

// ============================================================
// Per-slot memory
// ============================================================

struct SlotMem {
    input: DmaBuffer,
    dev: DmaBuffer,
    ep0: Ring,
    buf: DmaBuffer,
}

impl SlotMem {
    /// Input Context dword: context `ctx` (0 = control, 1 = slot, 1 + DCI
    /// = endpoint), dword `dw`
    unsafe fn input_dw(&self, ctx_size: usize, ctx: usize, dw: usize) -> *mut u32 {
        (self.input.virt.add(ctx * ctx_size) as *mut u32).add(dw)
    }

    unsafe fn clear_input(&self) {
        core::ptr::write_bytes(self.input.virt, 0, 4096);
    }

    unsafe fn buf(&self, len: usize) -> &[u8] {
        core::slice::from_raw_parts(self.buf.virt, len)
    }
}

const NO_SLOT: Option<SlotMem> = None;
static SLOT_MEM: super::UsbGlobal<[Option<SlotMem>; MAX_SLOTS + 1]> = super::UsbGlobal::new([NO_SLOT; MAX_SLOTS + 1]);

/// Root port of every slot that stays configured (hubs and keyboards);
/// 0 = not kept. Used to release everything behind a disconnected port.
static KEPT_ROOT_PORT: super::UsbGlobal<[u8; MAX_SLOTS + 1]> = super::UsbGlobal::new([0; MAX_SLOTS + 1]);

unsafe fn slot_mem(slot: u8) -> Result<&'static mut SlotMem> {
    let entry = &mut SLOT_MEM.get()[slot as usize];
    match entry {
        Some(m) => {
            m.clear_input();
            core::ptr::write_bytes(m.dev.virt, 0, 4096);
            core::ptr::write_bytes(m.buf.virt, 0, 4096);
            m.ep0.reset();
        }
        None => {
            *entry = Some(SlotMem { input: dma_page()?, dev: dma_page()?, ep0: Ring::new()?, buf: dma_page()? });
        }
    }
    Ok(entry.as_mut().unwrap())
}

// ============================================================
// Keyboards
// ============================================================

struct Keyboard {
    slot: u8,
    dci: u8,
    vid: u16,
    pid: u16,
    ring: Ring,
    report: DmaBuffer,
    active: bool,
    dead: bool,
    prev: [u8; 7],
}

const NO_KEYBOARD: Option<Keyboard> = None;
static KEYBOARDS: super::UsbGlobal<[Option<Keyboard>; MAX_KEYBOARDS]> = super::UsbGlobal::new([NO_KEYBOARD; MAX_KEYBOARDS]);

fn keyboards() -> &'static mut [Option<Keyboard>; MAX_KEYBOARDS] {
    unsafe { KEYBOARDS.get() }
}

/// Number of USB keyboards ready
pub fn keyboard_count() -> usize {
    keyboards().iter().filter(|k| k.is_some()).count()
}

// ============================================================
// Mass storage (bulk-only transport)
// ============================================================

/// What `usb_list` reports about a storage device
#[derive(Clone, Copy)]
pub struct StorageInfo {
    pub id: u32,
    pub vid: u16,
    pub pid: u16,
    pub subclass: u8,
    pub protocol: u8,
    pub speed: u8,
    pub root_port: u8,
    pub interface: u8,
    pub product: [u8; 48],
}

struct Storage {
    info: StorageInfo,
    slot: u8,
    dci_in: u8,
    dci_out: u8,
    /// Endpoint context dword 1 of each (type, burst, packet size), to add
    /// the endpoint again when it must be reset without being halted
    ep1_in: u32,
    ep1_out: u32,
    ep_in: u8,
    ep_out: u8,
    ring_in: Ring,
    ring_out: Ring,
    buf: DmaBuffer,
}

const NO_STORAGE: Option<Storage> = None;
static STORAGE: super::UsbGlobal<[Option<Storage>; MAX_STORAGE]> = super::UsbGlobal::new([NO_STORAGE; MAX_STORAGE]);
/// Ids are never reused, so a handle to an unplugged stick cannot reach
/// the next device in its place
static NEXT_STORAGE_ID: super::UsbGlobal<u32> = super::UsbGlobal::new(1);

fn storage() -> &'static mut [Option<Storage>; MAX_STORAGE] {
    unsafe { STORAGE.get() }
}

/// The storage devices present
pub fn storage_list() -> alloc::vec::Vec<StorageInfo> {
    storage().iter().flatten().map(|s| s.info).collect()
}

pub fn storage_exists(id: u32) -> bool {
    storage().iter().flatten().any(|s| s.info.id == id)
}

fn storage_by_id(id: u32) -> Option<&'static mut Storage> {
    storage().iter_mut().flatten().find(|s| s.info.id == id)
}

/// Errors of the storage calls, as the kernel's status codes
pub enum StorageError {
    Gone,
    Stall,
    Timeout,
    Io,
    Invalid,
}

fn storage_err(e: UsbError) -> StorageError {
    match e {
        UsbError::TransferFailed(CC_STALL) => StorageError::Stall,
        UsbError::Timeout => StorageError::Timeout,
        UsbError::NoController => StorageError::Gone,
        _ => StorageError::Io,
    }
}

/// Bulk IN: up to `data.len()` (at most BULK_MAX) bytes into `data`;
/// returns how many came
pub unsafe fn storage_bulk_in(id: u32, data: &mut [u8], ms: u64) -> core::result::Result<usize, StorageError> {
    let s = storage_by_id(id).ok_or(StorageError::Gone)?;
    if data.len() > BULK_MAX {
        return Err(StorageError::Invalid);
    }
    let c = xhci::controller().ok_or(StorageError::Gone)?;
    let (ctx, port) = ep_ctx(s.slot, s.dci_in);
    let n = c.bulk(s.slot, s.dci_in, &mut s.ring_in, s.buf.phys, data.len(), ms, ctx, port).map_err(storage_err)?;
    let n = core::cmp::min(n, data.len());
    core::ptr::copy_nonoverlapping(s.buf.virt, data.as_mut_ptr(), n);
    Ok(n)
}

/// Bulk OUT: `data` (at most BULK_MAX bytes); returns how many went
pub unsafe fn storage_bulk_out(id: u32, data: &[u8], ms: u64) -> core::result::Result<usize, StorageError> {
    let s = storage_by_id(id).ok_or(StorageError::Gone)?;
    if data.len() > BULK_MAX {
        return Err(StorageError::Invalid);
    }
    core::ptr::copy_nonoverlapping(data.as_ptr(), s.buf.virt, data.len());
    let c = xhci::controller().ok_or(StorageError::Gone)?;
    let (ctx, port) = ep_ctx(s.slot, s.dci_out);
    c.bulk(s.slot, s.dci_out, &mut s.ring_out, s.buf.phys, data.len(), ms, ctx, port).map_err(storage_err)
}

/// The endpoint's context in the Device Context the controller keeps, and
/// the root port of the slot (for the time-out report)
unsafe fn ep_ctx(slot: u8, dci: u8) -> (*const u32, usize) {
    static ZERO: [u32; 8] = [0; 8];
    let cs = match xhci::controller() {
        Some(c) => c.ctx_size,
        None => return (ZERO.as_ptr(), 0),
    };
    let port = KEPT_ROOT_PORT.get()[slot as usize] as usize;
    match SLOT_MEM.get()[slot as usize].as_ref() {
        Some(m) => (m.dev.virt.add(dci as usize * cs) as *const u32, port),
        None => (ZERO.as_ptr(), port),
    }
}

/// The last resort when a stick no longer answers even after a Bulk-Only
/// Reset: reset its root port and enumerate it again, as an unplug and
/// replug (Linux's usb_reset_device does the same). The device gets a new
/// id; the driver holding this one sees it gone, and devmgr starts a new
/// driver for the new one. Everything else behind that port (a hub, a
/// keyboard on it) is enumerated again too.
pub unsafe fn storage_reset(id: u32) -> core::result::Result<(), StorageError> {
    let s = storage_by_id(id).ok_or(StorageError::Gone)?;
    let port = KEPT_ROOT_PORT.get()[s.slot as usize] as usize;
    if port == 0 {
        return Err(StorageError::Invalid);
    }
    log("[USB] port ");
    log_dec(port as u64);
    log(": resetting a mass storage device that does not answer\n");
    release_root_port(port as u8);
    delay_ms(50);
    enumerate_root_port(port);
    Ok(())
}

/// Endpoint state from the Device Context the controller keeps: 1
/// running, 2 halted, 3 stopped, 4 error (xHCI 6.2.3)
unsafe fn ep_state(slot: u8, dci: u8) -> u32 {
    let cs = match xhci::controller() {
        Some(c) => c.ctx_size,
        None => return 0,
    };
    match SLOT_MEM.get()[slot as usize].as_ref() {
        Some(m) => core::ptr::read_volatile(m.dev.virt.add(dci as usize * cs) as *const u32) & 7,
        None => 0,
    }
}

/// Bring a bulk endpoint back to a clean, empty ring whatever state it is
/// in, and clear the device's halt: Reset Endpoint if halted, Stop
/// Endpoint if running, Set TR Dequeue, CLEAR_FEATURE(ENDPOINT_HALT) (BOT
/// recovery). The ring is emptied only once the controller has let go of
/// it, so both sides agree where it starts.
pub unsafe fn storage_clear_halt(id: u32, input: bool) -> core::result::Result<(), StorageError> {
    let s = storage_by_id(id).ok_or(StorageError::Gone)?;
    let (dci, ep) = if input { (s.dci_in, s.ep_in) } else { (s.dci_out, s.ep_out) };
    let ring = if input { &mut s.ring_in } else { &mut s.ring_out };
    let slot = s.slot;
    let c = xhci::controller().ok_or(StorageError::Gone)?;
    let ep1 = if input { s.ep1_in } else { s.ep1_out };
    let m = SLOT_MEM.get()[slot as usize].as_mut().ok_or(StorageError::Gone)?;
    match ep_state(slot, dci) {
        2 | 4 => {
            // Halted: Reset Endpoint also resets the host's data toggle /
            // sequence number, as CLEAR_FEATURE does on the device
            c.reset_endpoint(slot, dci).map_err(storage_err)?;
            ring.reset();
            c.set_tr_dequeue(slot, dci, ring.phys()).map_err(storage_err)?;
        }
        state => {
            // Not halted (after a time-out or a Bulk-Only Reset): Reset
            // Endpoint is not allowed, so stop it, then drop and add it
            // again (xHCI 4.6.6, as Linux's xhci_endpoint_reset), which
            // resets its toggle and points it at the emptied ring
            if state == 1 {
                c.stop_endpoint(slot, dci).map_err(storage_err)?;
            }
            ring.reset();
            let cs = c.ctx_size;
            m.clear_input();
            *m.input_dw(cs, 0, 0) = 1 << dci;          // drop
            *m.input_dw(cs, 0, 1) = 1 | 1 << dci;      // add (with the slot context)
            for k in 0..4 {
                *m.input_dw(cs, 1, k) = core::ptr::read_volatile((m.dev.virt as *const u32).add(k));
            }
            let ctx = 1 + dci as usize;
            *m.input_dw(cs, ctx, 0) = 0;
            *m.input_dw(cs, ctx, 1) = ep1;
            *m.input_dw(cs, ctx, 2) = ring.phys() as u32 | 1;
            *m.input_dw(cs, ctx, 3) = (ring.phys() >> 32) as u32;
            *m.input_dw(cs, ctx, 4) = 3072;
            let c = xhci::controller().ok_or(StorageError::Gone)?;
            c.configure_endpoint(slot, m.input.phys).map_err(storage_err)?;
        }
    }
    ctrl(slot, m, Setup { request_type: 0x02, request: REQ_CLEAR_FEATURE, value: FEATURE_ENDPOINT_HALT, index: ep as u16, length: 0 })
        .map_err(storage_err)?;
    Ok(())
}

/// A control request for the storage interface: class or standard
/// requests to the interface, its endpoints, or GET_DESCRIPTOR/GET_STATUS
/// to the device. IN data (at most 4096 bytes) or none. Returns the bytes
/// received.
pub unsafe fn storage_control(id: u32, setup: [u8; 8], data: &mut [u8]) -> core::result::Result<usize, StorageError> {
    let s = storage_by_id(id).ok_or(StorageError::Gone)?;
    let (rt, req) = (setup[0], setup[1]);
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let length = u16::from_le_bytes([setup[6], setup[7]]);
    let input = rt & 0x80 != 0;
    let ok = match rt & 0x1F {
        0 => rt & 0x60 == 0 && (req == REQ_GET_DESCRIPTOR || req == REQ_GET_STATUS),
        1 => index & 0xFF == s.info.interface as u16,
        2 => index & 0xFF == s.ep_in as u16 || index & 0xFF == s.ep_out as u16,
        _ => false,
    };
    if !ok || (length > 0 && !input) || length as usize > data.len() || length > 4096 {
        return Err(StorageError::Invalid);
    }
    let slot = s.slot;
    let m = SLOT_MEM.get()[slot as usize].as_mut().ok_or(StorageError::Gone)?;
    let n = ctrl(slot, m, Setup { request_type: rt, request: req, value, index, length }).map_err(storage_err)?;
    let n = core::cmp::min(n, length as usize);
    data[..n].copy_from_slice(m.buf(n));
    Ok(n)
}

struct BotInterface {
    interface: u8,
    subclass: u8,
    ep_in: u8,
    ep_out: u8,
    mps_in: u16,
    mps_out: u16,
    burst_in: u8,
    burst_out: u8,
}

/// The first mass-storage bulk-only interface (class 8, protocol 0x50)
/// with a bulk IN and a bulk OUT endpoint
fn find_bot(desc: &[u8]) -> Option<BotInterface> {
    let mut i = 0;
    let mut cur: Option<BotInterface> = None;
    let mut last_in = true;
    while i + 2 <= desc.len() {
        let len = desc[i] as usize;
        if len < 2 || i + len > desc.len() {
            break;
        }
        match desc[i + 1] {
            DT_INTERFACE if len >= 9 => {
                if let Some(b) = cur.take() {
                    if b.ep_in != 0 && b.ep_out != 0 {
                        return Some(b);
                    }
                }
                // Alternate setting 0 only
                if desc[i + 5] == CLASS_STORAGE && desc[i + 7] == 0x50 && desc[i + 3] == 0 {
                    cur = Some(BotInterface { interface: desc[i + 2], subclass: desc[i + 6], ep_in: 0, ep_out: 0, mps_in: 0, mps_out: 0, burst_in: 0, burst_out: 0 });
                }
            }
            DT_ENDPOINT if len >= 7 => {
                if let Some(b) = cur.as_mut() {
                    let addr = desc[i + 2];
                    let mps = ((desc[i + 5] as u16) << 8 | desc[i + 4] as u16) & 0x7FF;
                    if desc[i + 3] & 3 == 2 {
                        if addr & 0x80 != 0 && b.ep_in == 0 {
                            b.ep_in = addr;
                            b.mps_in = mps;
                            last_in = true;
                        } else if addr & 0x80 == 0 && b.ep_out == 0 {
                            b.ep_out = addr;
                            b.mps_out = mps;
                            last_in = false;
                        }
                    }
                }
            }
            DT_SS_EP_COMPANION if len >= 6 => {
                if let Some(b) = cur.as_mut() {
                    if last_in { b.burst_in = desc[i + 2]; } else { b.burst_out = desc[i + 2]; }
                }
            }
            _ => {}
        }
        i += len;
    }
    cur.filter(|b| b.ep_in != 0 && b.ep_out != 0)
}

/// A string descriptor as ASCII (English), or empty
unsafe fn read_string(slot: u8, m: &mut SlotMem, index: u8, out: &mut [u8; 48]) {
    if index == 0 {
        return;
    }
    let n = match ctrl(slot, m, Setup { request_type: 0x80, request: REQ_GET_DESCRIPTOR, value: (DT_STRING as u16) << 8 | index as u16, index: 0x0409, length: 255 }) {
        Ok(n) => n,
        Err(_) => return,
    };
    let b = m.buf(n);
    let len = core::cmp::min(b.first().copied().unwrap_or(0) as usize, n);
    let mut o = 0;
    let mut k = 2;
    while k + 1 < len && o < out.len() - 1 {
        let c = b[k] as u16 | (b[k + 1] as u16) << 8;
        out[o] = if (0x20..0x7F).contains(&c) { c as u8 } else { b'?' };
        o += 1;
        k += 2;
    }
    while o > 0 && out[o - 1] == b' ' {
        o -= 1;
        out[o] = 0;
    }
}

unsafe fn setup_storage(slot: u8, m: &mut SlotMem, loc: &Location, config_value: u8, bot: &BotInterface,
                        vid: u16, pid: u16, product_index: u8) -> Result<Kind> {
    let free = match storage().iter().position(|s| s.is_none()) {
        Some(i) => i,
        None => {
            indent(loc.depth);
            log("storage limit reached, skipping\n");
            return Ok(Kind::Other);
        }
    };
    let mut product = [0u8; 48];
    read_string(slot, m, product_index, &mut product);
    ctrl(slot, m, Setup { request_type: 0, request: REQ_SET_CONFIGURATION, value: config_value as u16, index: 0, length: 0 })?;
    let ring_in = Ring::new()?;
    let ring_out = Ring::new()?;
    let buf = crate::mm::dma::dma_alloc_aligned(BULK_MAX, BULK_MAX, crate::mm::dma::DmaZone::Below4G).map_err(|_| UsbError::NoMemory)?;
    let dci_in = (bot.ep_in & 0x0F) * 2 + 1;
    let dci_out = (bot.ep_out & 0x0F) * 2;
    let c = xhci::controller().ok_or(UsbError::NoController)?;
    let cs = c.ctx_size;
    m.clear_input();
    *m.input_dw(cs, 0, 1) = 1 | 1 << dci_in | 1 << dci_out;
    write_slot_ctx(m, cs, loc, core::cmp::max(dci_in, dci_out) as u32, None);
    let ep1 = |ty: u32, mps: u16, burst: u8| 3 << 1 | ty << 3 | (burst as u32) << 8 | (mps as u32) << 16; // CErr=3, Bulk IN (6) / OUT (2)
    let (ep1_in, ep1_out) = (ep1(6, bot.mps_in, bot.burst_in), ep1(2, bot.mps_out, bot.burst_out));
    for (dci, dw1, ring) in [(dci_in, ep1_in, &ring_in), (dci_out, ep1_out, &ring_out)] {
        let ctx = 1 + dci as usize;
        *m.input_dw(cs, ctx, 0) = 0;
        *m.input_dw(cs, ctx, 1) = dw1;
        *m.input_dw(cs, ctx, 2) = ring.phys() as u32 | 1;
        *m.input_dw(cs, ctx, 3) = (ring.phys() >> 32) as u32;
        *m.input_dw(cs, ctx, 4) = 3072; // average TRB length (xHCI 4.14.1.1: bulk)
    }
    c.configure_endpoint(slot, m.input.phys)?;
    let id = *NEXT_STORAGE_ID.get();
    *NEXT_STORAGE_ID.get() = id.wrapping_add(1).max(1);
    let info = StorageInfo {
        id,
        vid,
        pid,
        subclass: bot.subclass,
        protocol: 0x50,
        speed: loc.speed as u8,
        root_port: loc.root_port,
        interface: bot.interface,
        product,
    };
    storage()[free] = Some(Storage { info, slot, dci_in, dci_out, ep1_in, ep1_out, ep_in: bot.ep_in, ep_out: bot.ep_out, ring_in, ring_out, buf });
    indent(loc.depth);
    log("mass storage ");
    log_hex(vid as u64, 4);
    log(":");
    log_hex(pid as u64, 4);
    log(" ready\n");
    Ok(Kind::Storage)
}

/// Keep a transfer armed on every keyboard, decode finished reports and
/// handle root port changes.
pub unsafe fn poll() {
    let c = match xhci::controller() {
        Some(c) if !c.is_dead() => c,
        _ => return,
    };
    for kb in keyboards().iter_mut().flatten() {
        if !kb.active && !kb.dead {
            // Normal TRB, IOC + ISP
            kb.ring.push(kb.report.phys, REPORT_SIZE as u32, TRB_NORMAL << 10 | 1 << 5 | 1 << 2);
            c.ring_doorbell(kb.slot, kb.dci);
            kb.active = true;
        }
    }

    let mut changed: u64 = 0;
    for _ in 0..64 {
        let ev = match c.next_event() {
            Some(e) => e,
            None => break,
        };
        match (ev.control >> 10) & 0x3F {
            EVT_PORT_STATUS => {
                let port = ((ev.param >> 24) & 0xFF) as u32;
                if port > 0 && port < 64 {
                    changed |= 1 << port;
                }
            }
            EVT_TRANSFER => {
                let slot = (ev.control >> 24) as u8;
                let ep = ((ev.control >> 16) & 0x1F) as u8;
                let cc = (ev.status >> 24) as u8;
                let residual = (ev.status & 0xFF_FFFF) as usize;
                for kb in keyboards().iter_mut().flatten() {
                    if kb.slot != slot || kb.dci != ep {
                        continue;
                    }
                    kb.active = false;
                    if cc == CC_SUCCESS || cc == CC_SHORT_PACKET {
                        let len = REPORT_SIZE.saturating_sub(residual);
                        let report = core::slice::from_raw_parts(kb.report.virt as *const u8, len);
                        super::hid::process_report(report, &mut kb.prev);
                    } else {
                        // The endpoint halts on errors. CC=4 (transaction
                        // error) is what a disconnect looks like; the port
                        // change that follows removes the keyboard.
                        kb.dead = true;
                        if cc != 4 {
                            log("\n[USB] keyboard ");
                            log_hex(kb.vid as u64, 4);
                            log(":");
                            log_hex(kb.pid as u64, 4);
                            log(" stopped, CC=");
                            log_dec(cc as u64);
                            log("\n");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for port in 1..64usize {
        if changed & (1 << port) != 0 {
            root_port_changed(port);
        }
    }
}

unsafe fn root_port_changed(port: usize) {
    let c = match xhci::controller() {
        Some(c) => c,
        None => return,
    };
    if port > c.max_ports() {
        return;
    }
    let p = c.ack_port_changes(port);
    if p & PORTSC_CSC == 0 {
        return;
    }
    let released = release_root_port(port as u8);
    if p & PORTSC_CCS == 0 {
        if released > 0 {
            log("\n[USB] port ");
            log_dec(port as u64);
            log(" disconnected (");
            log_dec(keyboard_count() as u64);
            log(" USB keyboards)\n");
        }
        return;
    }
    delay_ms(100); // connect debounce
    let c = match xhci::controller() {
        Some(c) => c,
        None => return,
    };
    if c.portsc(port) & PORTSC_CCS == 0 {
        return;
    }
    log("\n[USB] port ");
    log_dec(port as u64);
    log(" connected\n");
    let before = keyboard_count();
    enumerate_root_port(port);
    if keyboard_count() != before {
        log("[USB] ");
        log_dec(keyboard_count() as u64);
        log(" USB keyboards\n");
    }
}

/// Disable every kept slot behind root port `port`; returns how many
unsafe fn release_root_port(port: u8) -> usize {
    let mut n = 0;
    for slot in 1..=MAX_SLOTS {
        if KEPT_ROOT_PORT.get()[slot] != port {
            continue;
        }
        for k in keyboards().iter_mut() {
            if k.as_ref().map_or(false, |k| k.slot as usize == slot) {
                *k = None;
            }
        }
        for s in storage().iter_mut() {
            if s.as_ref().map_or(false, |s| s.slot as usize == slot) {
                if let Some(old) = s.take() {
                    crate::mm::dma::dma_free(old.buf);
                }
            }
        }
        release_slot(slot as u8);
        n += 1;
    }
    n
}

unsafe fn release_slot(slot: u8) {
    if let Some(c) = xhci::controller() {
        let _ = c.disable_slot(slot);
        c.set_dcbaa(slot, 0);
    }
    KEPT_ROOT_PORT.get()[slot as usize] = 0;
}

// ============================================================
// Enumeration
// ============================================================

#[derive(Clone, Copy)]
struct Location {
    root_port: u8,
    /// Route string: hub port numbers below the root port
    route: u32,
    depth: u8,
    speed: u32,
    /// Transaction translator (high-speed hub slot and port) for full/low
    /// speed devices behind a high-speed hub; 0 = none
    tt_slot: u8,
    tt_port: u8,
    /// Address in two steps (the retry for devices that time out on a
    /// plain Address Device): BSR first, the device descriptor's first 8
    /// bytes at address 0, then SET_ADDRESS (as Linux's "new scheme")
    two_step: bool,
}

enum Kind {
    Keyboard,
    Hub,
    Storage,
    Other,
}

/// Enumerate every root port; returns the number of keyboards
pub unsafe fn enumerate_all() -> usize {
    let ports = match xhci::controller() {
        Some(c) => c.max_ports(),
        None => return 0,
    };
    for port in 1..=ports {
        enumerate_root_port(port);
        if xhci::controller().map_or(true, |c| c.is_dead()) {
            break;
        }
    }
    keyboard_count()
}

fn speed_name(speed: u32) -> &'static str {
    match speed {
        SPEED_FULL => "full",
        SPEED_LOW => "low",
        SPEED_HIGH => "high",
        SPEED_SUPER => "super",
        5 => "super+",
        _ => "unknown",
    }
}

fn indent(depth: u8) {
    for _ in 0..=depth {
        log("  ");
    }
}

unsafe fn enumerate_root_port(port: usize) {
    let c = match xhci::controller() {
        Some(c) => c,
        None => return,
    };
    let p = c.portsc(port);
    if p & PORTSC_CCS == 0 {
        return;
    }
    // Clear old change bits so the next connect/disconnect raises an event
    c.ack_port_changes(port);
    log("[USB] Port ");
    log_dec(port as u64);
    log(": ");
    let speed = (p >> PORTSC_SPEED_SHIFT) & 0xF;
    if speed < SPEED_SUPER || p & PORTSC_PED == 0 {
        if c.reset_port(port).is_err() {
            log("port reset failed\n");
            return;
        }
    }
    let speed = (c.portsc(port) >> PORTSC_SPEED_SHIFT) & 0xF;
    log(speed_name(speed));
    log("-speed device\n");
    let loc = Location { root_port: port as u8, route: 0, depth: 0, speed, tt_slot: 0, tt_port: 0, two_step: false };
    if attach(loc).is_ok() {
        return;
    }
    // Some devices (seen: a USB stick on the test laptop) time out on the
    // first Address Device. Like Linux: reset the port again and address
    // in two steps.
    let c = match xhci::controller() {
        Some(c) if !c.is_dead() => c,
        _ => return,
    };
    if c.portsc(port) & PORTSC_CCS == 0 {
        return;
    }
    log("[USB] Port ");
    log_dec(port as u64);
    log(": PORTSC ");
    log_hex(c.portsc(port) as u64, 8);
    log("; resetting it and addressing in two steps\n");
    delay_ms(100);
    if speed < SPEED_SUPER && c.reset_port(port).is_err() {
        log("port reset failed\n");
        return;
    }
    let speed = (c.portsc(port) >> PORTSC_SPEED_SHIFT) & 0xF;
    let _ = attach(Location { speed, two_step: true, ..loc });
}

unsafe fn attach(loc: Location) -> Result<Kind> {
    let c = xhci::controller().ok_or(UsbError::NoController)?;
    let slot = c.enable_slot()?;
    match setup_device(slot, &loc) {
        Ok(Kind::Other) => {
            release_slot(slot);
            Ok(Kind::Other)
        }
        Ok(kind) => {
            KEPT_ROOT_PORT.get()[slot as usize] = loc.root_port;
            Ok(kind)
        }
        Err(e) => {
            indent(loc.depth);
            log("setup failed: ");
            log(super::error_name(e));
            log("\n");
            release_slot(slot);
            Err(e)
        }
    }
}

fn default_mps0(speed: u32) -> u32 {
    match speed {
        SPEED_LOW | SPEED_FULL => 8,
        SPEED_HIGH => 64,
        _ => 512,
    }
}

/// xHCI Interval for an interrupt endpoint (xHCI 6.2.3.6)
fn interval(speed: u32, b_interval: u8) -> u32 {
    let b = core::cmp::max(b_interval as u32, 1);
    match speed {
        SPEED_LOW | SPEED_FULL => {
            let mut n = 3;
            while n < 10 && (1u32 << (n + 1)) <= b * 8 {
                n += 1;
            }
            n
        }
        _ => core::cmp::min(b - 1, 15),
    }
}

/// Fill the Input Context's slot context for `loc`
unsafe fn write_slot_ctx(m: &SlotMem, cs: usize, loc: &Location, entries: u32, hub: Option<(u32, u32)>) {
    let mut d0 = entries << 27 | loc.speed << 20 | (loc.route & 0xF_FFFF);
    let mut d1 = (loc.root_port as u32) << 16;
    let mut d2 = loc.tt_slot as u32 | (loc.tt_port as u32) << 8;
    if let Some((ports, think_time)) = hub {
        d0 |= 1 << 26;
        d1 |= ports << 24;
        if loc.speed == SPEED_HIGH {
            d2 |= think_time << 16;
        }
    }
    *m.input_dw(cs, 1, 0) = d0;
    *m.input_dw(cs, 1, 1) = d1;
    *m.input_dw(cs, 1, 2) = d2;
    *m.input_dw(cs, 1, 3) = 0;
}

unsafe fn ctrl(slot: u8, m: &mut SlotMem, setup: Setup) -> Result<usize> {
    let c = xhci::controller().ok_or(UsbError::NoController)?;
    let buf = m.buf.phys;
    c.control(slot, &mut m.ep0, setup, buf)
}

fn get_descriptor(ty: u8, len: u16) -> Setup {
    Setup { request_type: 0x80, request: REQ_GET_DESCRIPTOR, value: (ty as u16) << 8, index: 0, length: len }
}

unsafe fn setup_device(slot: u8, loc: &Location) -> Result<Kind> {
    let c: &mut Controller = xhci::controller().ok_or(UsbError::NoController)?;
    let cs = c.ctx_size;
    let m = slot_mem(slot)?;

    // ---- Address Device ----
    c.set_dcbaa(slot, m.dev.phys);
    let mut mps0 = default_mps0(loc.speed);
    *m.input_dw(cs, 0, 1) = 0x3; // add Slot + EP0
    write_slot_ctx(m, cs, loc, 1, None);
    let ep0 = m.ep0.phys();
    *m.input_dw(cs, 2, 1) = 3 << 1 | 4 << 3 | mps0 << 16; // CErr=3, Control
    *m.input_dw(cs, 2, 2) = ep0 as u32 | 1; // dequeue, DCS=1
    *m.input_dw(cs, 2, 3) = (ep0 >> 32) as u32;
    *m.input_dw(cs, 2, 4) = 8; // average TRB length
    if loc.two_step {
        // Default state without an address, then the real packet size
        c.address_device(slot, m.input.phys, true)?;
        delay_ms(2);
        ctrl(slot, m, get_descriptor(DT_DEVICE, 8))?;
        let real = m.buf(8)[7] as u32;
        if loc.speed < SPEED_SUPER && real != 0 {
            mps0 = real;
        }
        // Now SET_ADDRESS: EP0 continues where its ring stands
        let (next, cycle) = m.ep0.next();
        m.clear_input();
        *m.input_dw(cs, 0, 1) = 0x3;
        write_slot_ctx(m, cs, loc, 1, None);
        *m.input_dw(cs, 2, 1) = 3 << 1 | 4 << 3 | mps0 << 16;
        *m.input_dw(cs, 2, 2) = next as u32 | cycle;
        *m.input_dw(cs, 2, 3) = (next >> 32) as u32;
        *m.input_dw(cs, 2, 4) = 8;
        let c = xhci::controller().ok_or(UsbError::NoController)?;
        c.address_device(slot, m.input.phys, false)?;
        delay_ms(10); // SET_ADDRESS recovery, generously
        indent(loc.depth);
        log("addressed in two steps\n");
    } else {
        c.address_device(slot, m.input.phys, false)?;
        delay_ms(2); // SET_ADDRESS recovery
    }

    // ---- Device descriptor ----
    ctrl(slot, m, get_descriptor(DT_DEVICE, 8))?;
    let real_mps0 = m.buf(8)[7] as u32;
    if loc.speed < SPEED_SUPER && real_mps0 != 0 && real_mps0 != mps0 {
        mps0 = real_mps0;
        let c = xhci::controller().ok_or(UsbError::NoController)?;
        *m.input_dw(cs, 0, 1) = 0x2; // EP0 only
        *m.input_dw(cs, 2, 1) = 3 << 1 | 4 << 3 | mps0 << 16;
        c.evaluate_context(slot, m.input.phys)?;
    }
    ctrl(slot, m, get_descriptor(DT_DEVICE, 18))?;
    let d = m.buf(18);
    let (vid, pid, class, protocol) = ((d[9] as u16) << 8 | d[8] as u16, (d[11] as u16) << 8 | d[10] as u16, d[4], d[6]);
    let product_index = d[15];
    indent(loc.depth);
    log("device ");
    log_hex(vid as u64, 4);
    log(":");
    log_hex(pid as u64, 4);
    log(" class ");
    log_hex(class as u64, 2);
    log("\n");

    // ---- Configuration descriptor ----
    ctrl(slot, m, get_descriptor(DT_CONFIG, 9))?;
    let total = core::cmp::min((m.buf(4)[3] as usize) << 8 | m.buf(4)[2] as usize, 4096);
    let got = ctrl(slot, m, get_descriptor(DT_CONFIG, total as u16))?;
    let mut cfg_copy = [0u8; 512];
    let n = core::cmp::min(core::cmp::min(got, total), cfg_copy.len());
    cfg_copy[..n].copy_from_slice(m.buf(n));
    let cfg = &cfg_copy[..n];
    let config_value = if cfg.len() > 5 { cfg[5] } else { 1 };

    // ---- Hub ----
    if class == CLASS_HUB || has_interface_class(cfg, CLASS_HUB) {
        if loc.speed >= SPEED_SUPER {
            indent(loc.depth);
            log("USB 3 hub: its USB 2 side carries the keyboards, skipping\n");
            return Ok(Kind::Other);
        }
        if loc.depth >= MAX_HUB_DEPTH {
            indent(loc.depth);
            log("hub nested too deep, skipping\n");
            return Ok(Kind::Other);
        }
        setup_hub(slot, m, loc, config_value, protocol)?;
        return Ok(Kind::Hub);
    }

    // ---- Keyboard, or mass storage ----
    let kbd = match find_boot_keyboard(cfg) {
        Some(k) => k,
        None => {
            if let Some(bot) = find_bot(cfg) {
                return setup_storage(slot, m, loc, config_value, &bot, vid, pid, product_index);
            }
            indent(loc.depth);
            log("not a boot keyboard or USB storage, skipping\n");
            return Ok(Kind::Other);
        }
    };
    let free = match keyboards().iter().position(|k| k.is_none()) {
        Some(i) => i,
        None => {
            indent(loc.depth);
            log("keyboard limit reached, skipping\n");
            return Ok(Kind::Other);
        }
    };
    ctrl(slot, m, Setup { request_type: 0, request: REQ_SET_CONFIGURATION, value: config_value as u16, index: 0, length: 0 })?;
    let iface = kbd.interface as u16;
    if ctrl(slot, m, Setup { request_type: 0x21, request: REQ_SET_PROTOCOL, value: 0, index: iface, length: 0 }).is_err() {
        indent(loc.depth);
        log("SET_PROTOCOL failed (continuing)\n");
    }
    let _ = ctrl(slot, m, Setup { request_type: 0x21, request: REQ_SET_IDLE, value: 0, index: iface, length: 0 });

    // Interrupt IN endpoint
    let ring = Ring::new()?;
    let report = dma_page()?;
    let dci = (kbd.endpoint & 0x0F) as u32 * 2 + 1;
    let mps = kbd.max_packet as u32;
    m.clear_input();
    *m.input_dw(cs, 0, 1) = 1 | 1 << dci;
    write_slot_ctx(m, cs, loc, dci, None);
    let ctx = 1 + dci as usize;
    *m.input_dw(cs, ctx, 0) = interval(loc.speed, kbd.interval) << 16;
    *m.input_dw(cs, ctx, 1) = 3 << 1 | 7 << 3 | mps << 16; // CErr=3, Interrupt IN
    *m.input_dw(cs, ctx, 2) = ring.phys() as u32 | 1;
    *m.input_dw(cs, ctx, 3) = (ring.phys() >> 32) as u32;
    *m.input_dw(cs, ctx, 4) = mps << 16 | 8; // max ESIT payload, average TRB length
    let c = xhci::controller().ok_or(UsbError::NoController)?;
    c.configure_endpoint(slot, m.input.phys)?;

    keyboards()[free] = Some(Keyboard {
        slot,
        dci: dci as u8,
        vid,
        pid,
        ring,
        report,
        active: false,
        dead: false,
        prev: [0; 7],
    });
    indent(loc.depth);
    log("keyboard ");
    log_hex(vid as u64, 4);
    log(":");
    log_hex(pid as u64, 4);
    log(" ready\n");
    Ok(Kind::Keyboard)
}

unsafe fn setup_hub(slot: u8, m: &mut SlotMem, loc: &Location, config_value: u8, protocol: u8) -> Result<()> {
    ctrl(slot, m, Setup { request_type: 0, request: REQ_SET_CONFIGURATION, value: config_value as u16, index: 0, length: 0 })?;
    let n = ctrl(slot, m, Setup { request_type: 0xA0, request: REQ_GET_DESCRIPTOR, value: (DT_HUB as u16) << 8, index: 0, length: 16 })?;
    if n < 7 {
        return Err(UsbError::NotSupported);
    }
    let hd = m.buf(7);
    let ports = hd[2] as u32;
    let think_time = (((hd[4] as u32) << 8 | hd[3] as u32) >> 5) & 3;
    let power_good_ms = hd[5] as u64 * 2;
    indent(loc.depth);
    log("hub with ");
    log_dec(ports as u64);
    log(if protocol == 2 { " ports (multi-TT, using one TT)\n" } else { " ports\n" });

    // Tell the controller this slot is a hub
    let cs = xhci::controller().ok_or(UsbError::NoController)?.ctx_size;
    m.clear_input();
    *m.input_dw(cs, 0, 1) = 1; // slot context only
    write_slot_ctx(m, cs, loc, 1, Some((ports, think_time)));
    let c = xhci::controller().ok_or(UsbError::NoController)?;
    if c.configure_endpoint(slot, m.input.phys).is_err() {
        let c = xhci::controller().ok_or(UsbError::NoController)?;
        c.evaluate_context(slot, m.input.phys)?;
    }

    for p in 1..=ports {
        let _ = ctrl(slot, m, hub_feature(REQ_SET_FEATURE, HUB_PORT_POWER, p));
    }
    delay_ms(core::cmp::max(power_good_ms, 20) + 100);

    for p in 1..=ports {
        let (status, _) = match hub_port_status(slot, m, p) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if status & HUB_STAT_CONNECTION == 0 {
            continue;
        }
        let _ = ctrl(slot, m, hub_feature(REQ_CLEAR_FEATURE, HUB_C_PORT_CONNECTION, p));
        indent(loc.depth);
        log("hub port ");
        log_dec(p as u64);
        log(": ");
        if ctrl(slot, m, hub_feature(REQ_SET_FEATURE, HUB_PORT_RESET, p)).is_err() {
            log("reset request failed\n");
            continue;
        }
        let mut reset_done = false;
        for _ in 0..50 {
            delay_ms(10);
            if let Ok((_, change)) = hub_port_status(slot, m, p) {
                if change & HUB_CHANGE_RESET != 0 {
                    reset_done = true;
                    break;
                }
            }
        }
        let _ = ctrl(slot, m, hub_feature(REQ_CLEAR_FEATURE, HUB_C_PORT_RESET, p));
        if !reset_done {
            log("reset timed out\n");
            continue;
        }
        delay_ms(10);
        let status = match hub_port_status(slot, m, p) {
            Ok((s, _)) => s,
            Err(_) => continue,
        };
        if status & HUB_STAT_ENABLE == 0 {
            log("not enabled after reset\n");
            continue;
        }
        let speed = if status & HUB_STAT_LOW_SPEED != 0 {
            SPEED_LOW
        } else if status & HUB_STAT_HIGH_SPEED != 0 {
            SPEED_HIGH
        } else {
            SPEED_FULL
        };
        log(speed_name(speed));
        log("-speed device\n");
        let (tt_slot, tt_port) = if speed == SPEED_HIGH {
            (0, 0)
        } else if loc.speed == SPEED_HIGH {
            (slot, p as u8)
        } else {
            (loc.tt_slot, loc.tt_port)
        };
        let child = Location {
            root_port: loc.root_port,
            route: loc.route | core::cmp::min(p, 15) << (4 * loc.depth as u32),
            depth: loc.depth + 1,
            speed,
            tt_slot,
            tt_port,
            two_step: false,
        };
        let _ = attach(child);
        if xhci::controller().map_or(true, |c| c.is_dead()) {
            break;
        }
    }
    Ok(())
}

fn hub_feature(request: u8, feature: u16, port: u32) -> Setup {
    Setup { request_type: 0x23, request, value: feature, index: port as u16, length: 0 }
}

unsafe fn hub_port_status(slot: u8, m: &mut SlotMem, port: u32) -> Result<(u16, u16)> {
    let n = ctrl(slot, m, Setup { request_type: 0xA3, request: REQ_GET_STATUS, value: 0, index: port as u16, length: 4 })?;
    if n < 4 {
        return Err(UsbError::NotSupported);
    }
    let b = m.buf(4);
    Ok(((b[1] as u16) << 8 | b[0] as u16, (b[3] as u16) << 8 | b[2] as u16))
}

fn has_interface_class(desc: &[u8], class: u8) -> bool {
    let mut i = 0;
    while i + 2 <= desc.len() {
        let len = desc[i] as usize;
        if len < 2 || i + len > desc.len() {
            break;
        }
        if desc[i + 1] == DT_INTERFACE && len >= 9 && desc[i + 5] == class {
            return true;
        }
        i += len;
    }
    false
}

struct BootKeyboard {
    interface: u8,
    endpoint: u8,
    max_packet: u16,
    interval: u8,
}

/// First HID boot keyboard interface (class 3/1/1) and its interrupt IN
/// endpoint
fn find_boot_keyboard(desc: &[u8]) -> Option<BootKeyboard> {
    let mut i = 0;
    let mut iface: Option<u8> = None;
    while i + 2 <= desc.len() {
        let len = desc[i] as usize;
        if len < 2 || i + len > desc.len() {
            break;
        }
        match desc[i + 1] {
            DT_INTERFACE if len >= 9 => {
                iface = if desc[i + 5] == CLASS_HID && desc[i + 6] == 1 && desc[i + 7] == 1 {
                    Some(desc[i + 2])
                } else {
                    None
                };
            }
            DT_ENDPOINT if len >= 7 => {
                if let Some(interface) = iface {
                    let addr = desc[i + 2];
                    if addr & 0x80 != 0 && desc[i + 3] & 3 == 3 {
                        return Some(BootKeyboard {
                            interface,
                            endpoint: addr,
                            max_packet: ((desc[i + 5] as u16) << 8 | desc[i + 4] as u16) & 0x7FF,
                            interval: desc[i + 6],
                        });
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    None
}
