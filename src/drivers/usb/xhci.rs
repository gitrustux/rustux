// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! xHCI host controller: bring-up, commands, control transfers, events.
//!
//! Hardware lessons carried over from `kernel-efi` (see the loader README):
//! DBOFF is capability offset 0x14; delays and timeouts are in real time;
//! a stuck command is aborted by writing CA together with the address of
//! the *next* command (so a controller that latches the pointer resumes in
//! the right place) and device-independent commands are retried once;
//! completions are matched to their command by TRB address.

use super::ring::{dma_page, EventRing, Ring, Trb};
use super::{log, log_dec, log_hex, Result, UsbError};
use crate::arch::amd64::mm::kernel_map;
use crate::arch::amd64::tsc::{delay_ms, Deadline};
use crate::mm::dma::DmaBuffer;

// Capability registers
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_HCSPARAMS2: usize = 0x08;
const CAP_HCCPARAMS1: usize = 0x10;
const CAP_DBOFF: usize = 0x14;
const CAP_RTSOFF: usize = 0x18;

// Operational registers
const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_CRCR: usize = 0x18;
const OP_DCBAAP: usize = 0x30;
const OP_CONFIG: usize = 0x38;
const OP_PORTSC: usize = 0x400;

const CMD_RUN: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const STS_HCH: u32 = 1 << 0;
const STS_HSE: u32 = 1 << 2;
const STS_EINT: u32 = 1 << 3;
const STS_PCD: u32 = 1 << 4;
const STS_CNR: u32 = 1 << 11;
const STS_HCE: u32 = 1 << 12;
const CRCR_CA: u64 = 1 << 2;
const CRCR_CRR: u32 = 1 << 3;

// Interrupter 0 (runtime base + 0x20)
const IR_IMAN: usize = 0x20;
const IR_IMOD: usize = 0x24;
const IR_ERSTSZ: usize = 0x28;
const IR_ERSTBA: usize = 0x30;
const IR_ERDP: usize = 0x38;
const IMAN_IP: u32 = 1 << 0;
const IMAN_IE: u32 = 1 << 1;
const ERDP_EHB: u64 = 1 << 3;

// PORTSC
pub const PORTSC_CCS: u32 = 1 << 0;
pub const PORTSC_PED: u32 = 1 << 1;
pub const PORTSC_PR: u32 = 1 << 4;
pub const PORTSC_PP: u32 = 1 << 9;
pub const PORTSC_SPEED_SHIFT: u32 = 10;
pub const PORTSC_CSC: u32 = 1 << 17;
pub const PORTSC_PRC: u32 = 1 << 21;
/// All RW1C change bits (CSC, PEC, WRC, OCC, PRC, PLC, CEC)
pub const PORTSC_CHANGE_BITS: u32 = 0x00FE_0000;
/// Bits that keep their value when written back; everything else must be
/// written as 0 so the port is not disabled or changes cleared by accident
pub const PORTSC_PRESERVE: u32 = PORTSC_PP | (3 << 14) | (7 << 25);

/// Protocol speed IDs
pub const SPEED_FULL: u32 = 1;
pub const SPEED_LOW: u32 = 2;
pub const SPEED_HIGH: u32 = 3;
pub const SPEED_SUPER: u32 = 4;

/// TRB types
pub const TRB_NORMAL: u32 = 1;
pub const TRB_SETUP: u32 = 2;
pub const TRB_DATA: u32 = 3;
pub const TRB_STATUS: u32 = 4;
pub const TRB_ENABLE_SLOT: u32 = 9;
pub const TRB_DISABLE_SLOT: u32 = 10;
pub const TRB_ADDRESS_DEVICE: u32 = 11;
pub const TRB_CONFIGURE_EP: u32 = 12;
pub const TRB_EVALUATE_CTX: u32 = 13;
pub const TRB_RESET_EP: u32 = 14;
pub const TRB_STOP_EP: u32 = 15;
pub const TRB_SET_TR_DEQUEUE: u32 = 16;
pub const EVT_TRANSFER: u32 = 32;
pub const EVT_COMMAND: u32 = 33;
pub const EVT_PORT_STATUS: u32 = 34;

/// Completion codes
pub const CC_SUCCESS: u8 = 1;
pub const CC_STALL: u8 = 6;
pub const CC_SHORT_PACKET: u8 = 13;

/// Events set aside while waiting for a particular one (see `stash`)
const STASH: usize = 64;

/// Slots enabled (hubs keep theirs while devices behind them are set up)
pub const MAX_SLOTS: usize = 32;

/// USB control request (8-byte setup packet)
#[derive(Clone, Copy)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    fn packed(&self) -> u64 {
        self.request_type as u64
            | (self.request as u64) << 8
            | (self.value as u64) << 16
            | (self.index as u64) << 32
            | (self.length as u64) << 48
    }
}

pub struct Controller {
    cap: u64,
    op: u64,
    rt: u64,
    db: u64,
    max_ports: usize,
    /// Context size in bytes (32, or 64 when HCCPARAMS1.CSZ is set)
    pub ctx_size: usize,
    dcbaa: DmaBuffer,
    cmd: Ring,
    events: EventRing,
    _scratch: Option<(DmaBuffer, alloc::vec::Vec<DmaBuffer>)>,
    dead: bool,
    /// Events that arrived while a command or transfer waited for its own
    /// completion: kept for `next_event` (keyboard reports, port changes)
    /// instead of being lost
    stash: [Trb; STASH],
    stashed: usize,
}

static CONTROLLER: super::UsbGlobal<Option<Controller>> = super::UsbGlobal::new(None);

/// # Safety
/// USB_LOCK must be held.
pub unsafe fn set_controller(c: Controller) {
    *CONTROLLER.get() = Some(c);
}

/// # Safety
/// USB_LOCK must be held.
pub unsafe fn controller() -> Option<&'static mut Controller> {
    CONTROLLER.get().as_mut()
}

fn alloc_table_page() -> Option<u64> {
    crate::mm::pmm::pmm_alloc_kernel_page().ok()
}

impl Controller {
    unsafe fn r32(&self, addr: u64) -> u32 {
        core::ptr::read_volatile(addr as *const u32)
    }
    unsafe fn w32(&self, addr: u64, v: u32) {
        core::ptr::write_volatile(addr as *mut u32, v)
    }
    unsafe fn op_r(&self, off: usize) -> u32 {
        self.r32(self.op + off as u64)
    }
    unsafe fn op_w(&self, off: usize, v: u32) {
        self.w32(self.op + off as u64, v)
    }
    unsafe fn op_w64(&self, off: usize, v: u64) {
        self.op_w(off, v as u32);
        self.op_w(off + 4, (v >> 32) as u32);
    }
    unsafe fn rt_w64(&self, off: usize, v: u64) {
        self.w32(self.rt + off as u64, v as u32);
        self.w32(self.rt + off as u64 + 4, (v >> 32) as u32);
    }

    unsafe fn wait_op(&self, off: usize, mask: u32, expected: u32, ms: u64) -> bool {
        let deadline = Deadline::after_ms(ms);
        loop {
            if self.op_r(off) & mask == expected {
                return true;
            }
            if deadline.expired() {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    /// Map, take over, reset and start the controller at `mmio_phys`
    pub unsafe fn new(mmio_phys: u64) -> Result<Self> {
        // The register space is 64 KiB on every controller seen so far;
        // map more if the runtime or doorbell registers lie beyond that.
        let mut size: u64 = 0x10000;
        let mut cap = kernel_map::map_mmio(mmio_phys, size, alloc_table_page).map_err(|_| UsbError::NoMemory)?;
        let first = core::ptr::read_volatile(cap as *const u32);
        if first == 0xFFFF_FFFF {
            log("[USB] Controller registers read as all ones\n");
            return Err(UsbError::InitFailed);
        }
        let cap_len = (first & 0xFF) as u64;
        let version = first >> 16;
        let rtsoff = (core::ptr::read_volatile((cap + CAP_RTSOFF as u64) as *const u32) & !0x1F) as u64;
        let dboff = (core::ptr::read_volatile((cap + CAP_DBOFF as u64) as *const u32) & !0x3) as u64;
        let need = core::cmp::max(rtsoff + 0x20 + 32 * 1024, dboff + 256 * 4);
        if need > size {
            size = need;
            cap = kernel_map::map_mmio(mmio_phys, size, alloc_table_page).map_err(|_| UsbError::NoMemory)?;
        }
        let hcs1 = core::ptr::read_volatile((cap + CAP_HCSPARAMS1 as u64) as *const u32);
        let hcs2 = core::ptr::read_volatile((cap + CAP_HCSPARAMS2 as u64) as *const u32);
        let hcc1 = core::ptr::read_volatile((cap + CAP_HCCPARAMS1 as u64) as *const u32);
        let max_slots = (hcs1 & 0xFF) as usize;
        let max_ports = ((hcs1 >> 24) & 0xFF) as usize;
        let scratchpads = (((hcs2 >> 21) & 0x1F) << 5 | ((hcs2 >> 27) & 0x1F)) as usize;
        let ctx_size = if hcc1 & (1 << 2) != 0 { 64 } else { 32 };

        log("[USB] xHCI ");
        log_hex(version as u64, 4);
        log(": ");
        log_dec(max_slots as u64);
        log(" slots, ");
        log_dec(max_ports as u64);
        log(" ports, ");
        log_dec(scratchpads as u64);
        log(" scratchpads, ");
        log_dec(ctx_size as u64);
        log("-byte contexts\n");

        let mut c = Controller {
            cap,
            op: cap + cap_len,
            rt: cap + rtsoff,
            db: cap + dboff,
            max_ports,
            ctx_size,
            dcbaa: dma_page()?,
            cmd: Ring::new()?,
            events: EventRing::new()?,
            _scratch: None,
            dead: false,
            stash: [Trb::default(); STASH],
            stashed: 0,
        };
        c.bios_handoff(hcc1);
        c.reset()?;
        c.start(max_slots, scratchpads)?;
        Ok(c)
    }

    /// Take ownership from the firmware (USB Legacy Support capability)
    unsafe fn bios_handoff(&self, hcc1: u32) {
        let mut off = (((hcc1 >> 16) & 0xFFFF) * 4) as u64;
        for _ in 0..64 {
            if off == 0 {
                return;
            }
            let reg = self.cap + off;
            let v = self.r32(reg);
            if v & 0xFF == 1 {
                if v & (1 << 16) != 0 {
                    self.w32(reg, v | (1 << 24));
                    let deadline = Deadline::after_ms(1000);
                    while self.r32(reg) & (1 << 16) != 0 && !deadline.expired() {
                        delay_ms(1);
                    }
                    let now = self.r32(reg);
                    if now & (1 << 16) != 0 {
                        self.w32(reg, (now & !(1 << 16)) | (1 << 24));
                        log("[USB] BIOS did not release the controller, taking it\n");
                    }
                } else {
                    self.w32(reg, v | (1 << 24));
                }
                // USBLEGCTLSTS: SMIs off, clear SMI status
                let ctl = reg + 4;
                let s = self.r32(ctl);
                self.w32(ctl, (s & 0xE000_0000) & !0x0000_E011);
                return;
            }
            let next = ((v >> 8) & 0xFF) as u64;
            if next == 0 {
                return;
            }
            off += next * 4;
        }
    }

    unsafe fn reset(&mut self) -> Result<()> {
        let cmd = self.op_r(OP_USBCMD);
        if cmd & CMD_RUN != 0 {
            self.op_w(OP_USBCMD, cmd & !CMD_RUN);
        }
        if !self.wait_op(OP_USBSTS, STS_HCH, STS_HCH, 100) {
            log("[USB] Controller did not halt\n");
            return Err(UsbError::InitFailed);
        }
        self.op_w(OP_USBCMD, CMD_HCRST);
        delay_ms(1);
        if !self.wait_op(OP_USBCMD, CMD_HCRST, 0, 1000) || !self.wait_op(OP_USBSTS, STS_CNR, 0, 1000) {
            log("[USB] Controller reset did not complete\n");
            return Err(UsbError::InitFailed);
        }
        Ok(())
    }

    unsafe fn start(&mut self, max_slots: usize, scratchpads: usize) -> Result<()> {
        self.op_w(OP_CONFIG, core::cmp::min(max_slots, MAX_SLOTS) as u32);

        // Scratchpad pages, if the controller wants them: DCBAA[0] points
        // to an array of their physical addresses
        if scratchpads > 0 {
            let array = dma_page()?;
            let mut pages = alloc::vec::Vec::with_capacity(scratchpads);
            for i in 0..core::cmp::min(scratchpads, 512) {
                let p = dma_page()?;
                core::ptr::write_volatile((array.virt as *mut u64).add(i), p.phys);
                pages.push(p);
            }
            core::ptr::write_volatile(self.dcbaa.virt as *mut u64, array.phys);
            self._scratch = Some((array, pages));
        }
        self.op_w64(OP_DCBAAP, self.dcbaa.phys);

        // Command ring (CRCR must be written while halted)
        self.op_w64(OP_CRCR, self.cmd.phys() | 1);

        // Event ring for interrupter 0; writing ERSTBA enables it
        self.w32(self.rt + IR_ERSTSZ as u64, 1);
        self.rt_w64(IR_ERDP, self.events.phys());
        self.rt_w64(IR_ERSTBA, self.events.erst.phys);
        self.w32(self.rt + IR_IMOD as u64, 0);
        self.w32(self.rt + IR_IMAN as u64, IMAN_IP | IMAN_IE);

        self.op_w(OP_USBCMD, CMD_RUN);
        if !self.wait_op(OP_USBSTS, STS_HCH, 0, 100) {
            log("[USB] Controller did not start\n");
            return Err(UsbError::InitFailed);
        }
        if self.op_r(OP_USBSTS) & (STS_HSE | STS_HCE) != 0 {
            log("[USB] Host system / controller error\n");
            return Err(UsbError::InitFailed);
        }

        // Power every port, then let devices connect and debounce
        for port in 1..=self.max_ports {
            let p = self.portsc(port);
            if p & PORTSC_PP == 0 {
                self.set_portsc(port, (p & PORTSC_PRESERVE) | PORTSC_PP);
            }
        }
        delay_ms(200);
        Ok(())
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    pub fn max_ports(&self) -> usize {
        self.max_ports
    }

    /// PORTSC of root port `port` (1-based)
    pub unsafe fn portsc(&self, port: usize) -> u32 {
        if port == 0 || port > self.max_ports {
            return 0;
        }
        self.op_r(OP_PORTSC + (port - 1) * 0x10)
    }

    pub unsafe fn set_portsc(&self, port: usize, v: u32) {
        if port == 0 || port > self.max_ports {
            return;
        }
        self.op_w(OP_PORTSC + (port - 1) * 0x10, v)
    }

    /// Acknowledge (clear) the port's change bits
    pub unsafe fn ack_port_changes(&self, port: usize) -> u32 {
        let p = self.portsc(port);
        self.set_portsc(port, (p & PORTSC_PRESERVE) | (p & PORTSC_CHANGE_BITS));
        p
    }

    /// Reset a USB 2 root port and wait until it is enabled
    pub unsafe fn reset_port(&self, port: usize) -> Result<()> {
        let p = self.ack_port_changes(port);
        self.set_portsc(port, (p & PORTSC_PRESERVE) | PORTSC_PR);
        let deadline = Deadline::after_ms(500);
        let mut done = false;
        while !deadline.expired() {
            let p = self.portsc(port);
            if p & PORTSC_PRC != 0 && p & PORTSC_PR == 0 {
                done = true;
                break;
            }
            delay_ms(1);
        }
        let p = self.ack_port_changes(port);
        if !done {
            return Err(UsbError::Timeout);
        }
        if p & PORTSC_PED == 0 {
            return Err(UsbError::InitFailed);
        }
        delay_ms(10); // reset recovery (TRSTRCY)
        Ok(())
    }

    pub unsafe fn ring_doorbell(&self, slot: u8, target: u8) {
        self.w32(self.db + slot as u64 * 4, target as u32);
    }

    /// Next event, if any; advances ERDP and clears the pending flags
    pub unsafe fn poll_event(&mut self) -> Option<Trb> {
        let ev = self.events.pop()?;
        self.rt_w64(IR_ERDP, self.events.dequeue_phys() | ERDP_EHB);
        let iman = self.r32(self.rt + IR_IMAN as u64);
        if iman & IMAN_IP != 0 {
            self.w32(self.rt + IR_IMAN as u64, iman | IMAN_IP);
        }
        let sts = self.op_r(OP_USBSTS);
        if sts & (STS_EINT | STS_PCD) != 0 {
            self.op_w(OP_USBSTS, sts & (STS_EINT | STS_PCD));
        }
        Some(ev)
    }

    /// The next event for the keyboard/port-change loop: set-aside events
    /// first, then new ones
    pub unsafe fn next_event(&mut self) -> Option<Trb> {
        if self.stashed > 0 {
            let ev = self.stash[0];
            self.stash.copy_within(1..self.stashed, 0);
            self.stashed -= 1;
            return Some(ev);
        }
        self.poll_event()
    }

    /// Keep an event that is not the one being waited for (dropped if the
    /// stash is full; a keyboard then re-arms on its next report)
    fn set_aside(&mut self, ev: Trb) {
        if self.stashed < STASH {
            self.stash[self.stashed] = ev;
            self.stashed += 1;
        }
    }

    /// Physical address of the Device Context Base Address Array entry
    pub unsafe fn set_dcbaa(&self, slot: u8, phys: u64) {
        core::ptr::write_volatile((self.dcbaa.virt as *mut u64).add(slot as usize), phys);
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    unsafe fn wait_command(&mut self, trb_phys: u64, ms: u64) -> Result<(u8, u8)> {
        let deadline = Deadline::after_ms(ms);
        loop {
            match self.poll_event() {
                Some(ev) => {
                    if (ev.control >> 10) & 0x3F == EVT_COMMAND && ev.param & !0xF == trb_phys {
                        return Ok(((ev.status >> 24) as u8, (ev.control >> 24) as u8));
                    }
                    if (ev.control >> 10) & 0x3F != EVT_COMMAND {
                        self.set_aside(ev);
                    }
                }
                None if deadline.expired() => return Err(UsbError::Timeout),
                None => core::hint::spin_loop(),
            }
        }
    }

    /// Stop the command ring after a timeout (xHCI 4.6.1.2)
    unsafe fn abort_commands(&mut self) {
        let (next, cycle) = self.cmd.next();
        self.op_w64(OP_CRCR, (next & !0x3F) | CRCR_CA | cycle as u64);
        if !self.wait_op(OP_CRCR, CRCR_CRR, 0, 5000) {
            log("[USB] Command ring did not stop; giving up on the controller\n");
            self.dead = true;
            return;
        }
        let deadline = Deadline::after_ms(50);
        while !deadline.expired() {
            if self.poll_event().is_none() {
                core::hint::spin_loop();
            }
        }
    }

    /// Run a command (5 s timeout; aborted on timeout, retried once unless
    /// it talks to a device). Returns the slot id from the completion.
    pub unsafe fn command(&mut self, name: &str, param: u64, status: u32, control: u32) -> Result<u8> {
        let ty = (control >> 10) & 0x3F;
        let attempts = if ty == TRB_ADDRESS_DEVICE { 1 } else { 2 };
        for attempt in 0..attempts {
            if self.dead {
                return Err(UsbError::InitFailed);
            }
            let trb = self.cmd.push(param, status, control);
            self.ring_doorbell(0, 0);
            match self.wait_command(trb, 5000) {
                Ok((CC_SUCCESS, slot)) => return Ok(slot),
                Ok((cc, _)) => {
                    log("[USB] ");
                    log(name);
                    log(" failed, CC=");
                    log_dec(cc as u64);
                    log("\n");
                    return Err(UsbError::CommandFailed(cc));
                }
                Err(_) => {
                    log("[USB] ");
                    log(name);
                    log(if attempt + 1 < attempts { " timed out, retrying\n" } else { " timed out, aborted\n" });
                    self.abort_commands();
                }
            }
        }
        Err(UsbError::Timeout)
    }

    pub unsafe fn enable_slot(&mut self) -> Result<u8> {
        let slot = self.command("Enable Slot", 0, 0, TRB_ENABLE_SLOT << 10)?;
        if slot == 0 || slot as usize > MAX_SLOTS {
            return Err(UsbError::InitFailed);
        }
        Ok(slot)
    }

    pub unsafe fn disable_slot(&mut self, slot: u8) -> Result<()> {
        self.command("Disable Slot", 0, 0, (slot as u32) << 24 | TRB_DISABLE_SLOT << 10).map(|_| ())
    }

    /// Address Device; `bsr` (Block Set Address Request) sets the slot up
    /// without sending SET_ADDRESS (the device stays at address 0)
    pub unsafe fn address_device(&mut self, slot: u8, input_ctx: u64, bsr: bool) -> Result<()> {
        self.command("Address Device", input_ctx, 0, (slot as u32) << 24 | (bsr as u32) << 9 | TRB_ADDRESS_DEVICE << 10).map(|_| ())
    }

    pub unsafe fn configure_endpoint(&mut self, slot: u8, input_ctx: u64) -> Result<()> {
        self.command("Configure Endpoint", input_ctx, 0, (slot as u32) << 24 | TRB_CONFIGURE_EP << 10).map(|_| ())
    }

    pub unsafe fn evaluate_context(&mut self, slot: u8, input_ctx: u64) -> Result<()> {
        self.command("Evaluate Context", input_ctx, 0, (slot as u32) << 24 | TRB_EVALUATE_CTX << 10).map(|_| ())
    }

    /// Bring a halted endpoint (after a stall or error) back to Stopped
    pub unsafe fn reset_endpoint(&mut self, slot: u8, dci: u8) -> Result<()> {
        self.command("Reset Endpoint", 0, 0, (slot as u32) << 24 | (dci as u32) << 16 | TRB_RESET_EP << 10).map(|_| ())
    }

    /// Stop a running endpoint (after a transfer timed out)
    pub unsafe fn stop_endpoint(&mut self, slot: u8, dci: u8) -> Result<()> {
        self.command("Stop Endpoint", 0, 0, (slot as u32) << 24 | (dci as u32) << 16 | TRB_STOP_EP << 10).map(|_| ())
    }

    /// Point a stopped endpoint at `ring_phys` (cycle state 1)
    pub unsafe fn set_tr_dequeue(&mut self, slot: u8, dci: u8, ring_phys: u64) -> Result<()> {
        self.command("Set TR Dequeue", ring_phys | 1, 0, (slot as u32) << 24 | (dci as u32) << 16 | TRB_SET_TR_DEQUEUE << 10)
            .map(|_| ())
    }

    /// One bulk transfer of `len` bytes (at most 64 KiB, not crossing a
    /// 64 KiB boundary) at `phys` on endpoint `dci`. Returns the bytes
    /// moved; a stall comes back as TransferFailed(CC_STALL), and on a
    /// time-out the endpoint is stopped and its ring emptied.
    ///
    /// `ep_ctx` is the endpoint's context in the Device Context (for the
    /// report on a time-out). The completion is matched by the TRB it
    /// names, so an old event of this endpoint (the "stopped" event of an
    /// earlier time-out, a completion that came too late) is not taken for
    /// this transfer's.
    pub unsafe fn bulk(&mut self, slot: u8, dci: u8, ring: &mut Ring, phys: u64, len: usize, ms: u64,
                       ep_ctx: *const u32, port: usize) -> Result<usize> {
        // Normal TRB: interrupt on short packet and on completion
        let trb = ring.push(phys, len as u32, TRB_NORMAL << 10 | 1 << 5 | 1 << 2);
        self.ring_doorbell(slot, dci);
        match self.wait_transfer_trb(slot, dci, trb, ms) {
            Ok((CC_SUCCESS, residual)) | Ok((CC_SHORT_PACKET, residual)) => Ok(len.saturating_sub(residual as usize)),
            Ok((cc, _)) => {
                // The endpoint halted: the caller recovers (clear halt)
                Err(UsbError::TransferFailed(cc))
            }
            Err(e) => {
                // What the controller was doing: the endpoint's state and
                // where its dequeue pointer is (on our TRB: the device did
                // not take or give the data), the controller's status
                // (HSE/HCE: the controller itself failed), the port
                let state = core::ptr::read_volatile(ep_ctx) & 7;
                let deq = (core::ptr::read_volatile(ep_ctx.add(2)) as u64 | (core::ptr::read_volatile(ep_ctx.add(3)) as u64) << 32) & !0xF;
                log("[USB] bulk transfer timed out (slot ");
                log_dec(slot as u64);
                log(", endpoint ");
                log_dec(dci as u64);
                log(", ");
                log_dec(len as u64);
                log(" bytes): endpoint state ");
                log_dec(state as u64);
                log(", dequeue ");
                log_hex(deq, 8);
                log(if deq == trb { " (our TRB)" } else if deq == trb + 16 { " (past our TRB)" } else { " (elsewhere)" });
                log(", USBSTS ");
                log_hex(self.op_r(OP_USBSTS) as u64, 8);
                if port > 0 && port <= self.max_ports() {
                    log(", PORTSC ");
                    log_hex(self.portsc(port) as u64, 8);
                }
                log("\n");
                let _ = self.stop_endpoint(slot, dci);
                ring.reset();
                let _ = self.set_tr_dequeue(slot, dci, ring.phys());
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------
    // Control transfers
    // ------------------------------------------------------------------

    /// Wait for the Transfer Event of the TRB at `trb` (endpoint `dci` of
    /// `slot`): (completion code, residual bytes). Other events of this
    /// endpoint are old ones and are dropped; other endpoints' are kept.
    pub unsafe fn wait_transfer_trb(&mut self, slot: u8, dci: u8, trb: u64, ms: u64) -> Result<(u8, u32)> {
        let deadline = Deadline::after_ms(ms);
        loop {
            match self.poll_event() {
                Some(ev) => {
                    let ty = (ev.control >> 10) & 0x3F;
                    if ty == EVT_TRANSFER && (ev.control >> 24) as u8 == slot && ((ev.control >> 16) & 0x1F) as u8 == dci {
                        if ev.param & !0xF == trb {
                            return Ok(((ev.status >> 24) as u8, ev.status & 0xFF_FFFF));
                        }
                        continue;
                    }
                    if ty != EVT_COMMAND {
                        self.set_aside(ev);
                    }
                }
                None if deadline.expired() => return Err(UsbError::Timeout),
                None => core::hint::spin_loop(),
            }
        }
    }

    /// Wait for a Transfer Event for endpoint `dci` of `slot`:
    /// (completion code, residual bytes)
    pub unsafe fn wait_transfer(&mut self, slot: u8, dci: u8, ms: u64) -> Result<(u8, u32)> {
        let deadline = Deadline::after_ms(ms);
        loop {
            match self.poll_event() {
                Some(ev) => {
                    if (ev.control >> 10) & 0x3F == EVT_TRANSFER
                        && (ev.control >> 24) as u8 == slot
                        && ((ev.control >> 16) & 0x1F) as u8 == dci
                    {
                        return Ok(((ev.status >> 24) as u8, ev.status & 0xFF_FFFF));
                    }
                    if (ev.control >> 10) & 0x3F != EVT_COMMAND {
                        self.set_aside(ev);
                    }
                }
                None if deadline.expired() => return Err(UsbError::Timeout),
                None => core::hint::spin_loop(),
            }
        }
    }

    /// Control transfer with an optional IN data stage into `data_phys`.
    /// Returns the number of bytes received.
    pub unsafe fn control(&mut self, slot: u8, ep0: &mut Ring, setup: Setup, data_phys: u64) -> Result<usize> {
        let has_data = setup.length > 0;
        // Setup stage: IDT, TRT = 3 (IN data) or 0 (no data)
        let trt = if has_data { 3 << 16 } else { 0 };
        ep0.push(setup.packed(), 8, TRB_SETUP << 10 | trt | 1 << 6);
        if has_data {
            // Data stage: IN, ISP
            ep0.push(data_phys, setup.length as u32, TRB_DATA << 10 | 1 << 16 | 1 << 2);
        }
        // Status stage: opposite direction (OUT after IN data, IN otherwise), IOC
        let dir = if has_data { 0 } else { 1 << 16 };
        ep0.push(0, 0, TRB_STATUS << 10 | dir | 1 << 5);
        self.ring_doorbell(slot, 1);

        let mut received = setup.length as usize;
        loop {
            let (cc, residual) = self.wait_transfer(slot, 1, 2000)?;
            match cc {
                CC_SHORT_PACKET => received = (setup.length as usize).saturating_sub(residual as usize),
                CC_SUCCESS => return Ok(received),
                _ => return Err(UsbError::TransferFailed(cc)),
            }
        }
    }
}
