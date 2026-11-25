//! Trident CyberBlade XP-Ai1 (`1023:8820`, rev `0x82`) BladeXP 2D Graphics Engine (GE).
//!
//! Register offsets and sequences verified from Linux `drivers/video/fbdev/tridentfb.c`
//! (`is_xp()`, `xp_init_accel()`, `xp_fill_rect()`, `xp_copy_rect()`, `xp_wait_engine()`)
//! and X.Org `xf86-video-trident` (`xp_accel.c`, `trident_regs.h`, `trident_driver.c`).
//! See `docs/stage-g/TRIDENT-8820-REGS.md`.

use abi::vbe_abi::{
    VBE_ERR_HW_TIMEOUT, VBE_ERR_INVALID_MODE, VBE_ERR_INVALID_RECT, VBE_ERR_UNSUPPORTED,
    VBE_ROP_PATCOPY, VBE_ROP_SRCCOPY, VbeBlitReq, VbeFillReq, VbeModeDesc,
};

/// Trident Microsystems PCI Vendor ID.
pub const TRIDENT_PCI_VENDOR: u16 = 0x1023;

/// Trident CyberBlade XP-Ai1 PCI Device ID (`CYBERBLADEXPAi1`, rev `0x82`).
pub const TRIDENT_PCI_DEVICE_XP_AI1: u16 = 0x8820;

/// Number of 4 KiB pages to map for Trident PCI BAR1 MMIO (`0x20000` = 128 KiB).
pub const TRIDENT_MMIO_PAGES: u32 = 32;

// Verified BladeXP (`xp_*`) MMIO register offsets relative to PCI BAR1:
pub const XP_REG_STATUS: u32 = 0x2120; // 8-bit: bit 7 (0x80) = GE busy; write 0x00 to reset
pub const XP_REG_COMMAND: u32 = 0x2124; // 8-bit: write 0x01 (GE_BLT) to trigger
pub const XP_REG_OPER: u32 = 0x2125; // 8-bit: depth | stride_hint (| 0x40 post-fill)
pub const XP_REG_CLIP_CTL: u32 = 0x2126; // 8-bit: write 0x03 on init
pub const XP_REG_ROP: u32 = 0x2127; // 8-bit: Windows ROP3 (0xF0 = PATCOPY, 0xCC = SRCCOPY)
pub const XP_REG_DRAW_MODE: u32 = 0x2128; // 32-bit: 0x4000 SOLIDFILL, 0x0004 SCR2SCR (+ XNEG/YNEG)
pub const XP_REG_DST_XY: u32 = 0x2138; // 32-bit: OLDDST = (x << 16) | y
pub const XP_REG_SRC_XY: u32 = 0x213C; // 32-bit: OLDSRC = (sx << 16) | sy
pub const XP_REG_DIM_WH: u32 = 0x2140; // 32-bit: OLDDIM = (w << 16) | h (exact w/h, NOT w-1/h-1)
pub const XP_REG_SRC_OFFSET: u32 = 0x2150; // 32-bit: pitch_px << shift
pub const XP_REG_DST_OFFSET: u32 = 0x2154; // 32-bit: pitch_px << shift
pub const XP_REG_FG_COLOR: u32 = 0x2158; // 32-bit: replicated foreground color

// MMIO-mirrored VGA CRTC index/data registers for disabling PCIReg MMIO bit 0:
pub const XP_MMIO_CRTC_IDX: u32 = 0x03D4;
pub const XP_MMIO_CRTC_DATA: u32 = 0x03D5;

// Verified BladeXP command/flag constants:
pub const XP_STATUS_BUSY: u8 = 0x80;
pub const XP_CMD_GE_BLT: u8 = 0x01;
pub const XP_ROP_PATCOPY: u8 = 0xF0;
pub const XP_ROP_SRCCOPY: u8 = 0xCC;
pub const XP_MODE_SOLIDFILL: u32 = 0x0000_4000;
pub const XP_MODE_SCR2SCR: u32 = 0x0000_0004;
pub const XP_DIR_YNEG: u32 = 0x0000_0100;
pub const XP_DIR_XNEG: u32 = 0x0000_0200;

/// Bounded idle poll iterations before resetting GE and returning timeout.
pub const XP_IDLE_TIMEOUT_ITERS: u32 = 1_000_000;

/// PCI discovery result for a Trident CyberBlade XP-Ai1 controller.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TridentPciInfo {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub rev: u8,
    pub lfb_bar0_phys: u32,
    pub mmio_bar1_phys: u32,
}

/// Abstract MMIO register bus so BladeXP 2D GE sequences are 100% unit-testable on host.
pub trait MmioBus {
    fn read8(&self, offset: u32) -> u8;
    fn write8(&mut self, offset: u32, val: u8);
    fn read32(&self, offset: u32) -> u32;
    fn write32(&mut self, offset: u32, val: u32);
}

/// Abstract VGA Port I/O bus for enabling Trident GE + MMIO prior to MMIO access.
pub trait VgaPio {
    fn inb(&self, port: u16) -> u8;
    fn outb(&mut self, port: u16, val: u8);
}

/// Live Ring-0 MMIO bus backed by a JEMM-mapped flat linear base address.
pub struct LiveMmioBus {
    base: u32,
}

impl LiveMmioBus {
    #[inline]
    pub const fn new(base: u32) -> Self {
        Self { base }
    }
}

impl MmioBus for LiveMmioBus {
    #[inline]
    fn read8(&self, offset: u32) -> u8 {
        let addr = self.base.wrapping_add(offset) as *const u8;
        unsafe { core::ptr::read_volatile(addr) }
    }

    #[inline]
    fn write8(&mut self, offset: u32, val: u8) {
        let addr = self.base.wrapping_add(offset) as *mut u8;
        unsafe { core::ptr::write_volatile(addr, val) }
    }

    #[inline]
    fn read32(&self, offset: u32) -> u32 {
        let addr = self.base.wrapping_add(offset) as *const u32;
        unsafe { core::ptr::read_volatile(addr) }
    }

    #[inline]
    fn write32(&mut self, offset: u32, val: u32) {
        let addr = self.base.wrapping_add(offset) as *mut u32;
        unsafe { core::ptr::write_volatile(addr, val) }
    }
}

/// Common 2D accelerator trait implemented by hardware backends.
pub trait Accel2d {
    fn is_hw_ready(&self) -> bool;
    fn configure_mode<B: MmioBus, P: VgaPio>(
        &mut self,
        bus: &mut B,
        pio: &mut P,
        mode: &VbeModeDesc,
    ) -> Result<(), u32>;
    fn fill_rect<B: MmioBus>(
        &mut self,
        bus: &mut B,
        mode: &VbeModeDesc,
        req: &VbeFillReq,
    ) -> Result<(), u32>;
    fn bit_blt<B: MmioBus>(
        &mut self,
        bus: &mut B,
        mode: &VbeModeDesc,
        req: &VbeBlitReq,
    ) -> Result<(), u32>;
    fn sync<B: MmioBus>(&mut self, bus: &mut B) -> Result<(), u32>;
    fn release_hw<B: MmioBus>(&mut self, bus: &mut B) -> Result<(), u32>;
}

/// Trident CyberBlade XP-Ai1 (`1023:8820`) BladeXP 2D GE driver state.
#[derive(Copy, Clone, Debug)]
pub struct Trident2dEngine {
    pub hw_ready: bool,
    pub mmio_enabled: bool,
    pub eng_oper: u8,
    pub active_bpp: u8,
    pub active_pitch: u32,
}

impl Trident2dEngine {
    pub const fn new() -> Self {
        Self {
            hw_ready: false,
            mmio_enabled: false,
            eng_oper: 0,
            active_bpp: 0,
            active_pitch: 0,
        }
    }

    /// Replicate packed color across 32 bits for BladeXP `XP_REG_FG_COLOR (0x2158)`.
    #[inline]
    pub const fn replicate_color(color: u32, bpp: u8) -> u32 {
        match bpp {
            8 => {
                let c = color & 0xFF;
                c | (c << 8) | (c << 16) | (c << 24)
            }
            16 => {
                let c = color & 0xFFFF;
                c | (c << 16)
            }
            _ => color,
        }
    }

    /// Compute `(depth, shift, oper)` for `xp_init_accel(pitch_px, bpp)`.
    #[inline]
    pub const fn compute_mode_params(mode: &VbeModeDesc) -> Option<(u8, u32, u8, u32)> {
        let (depth, shift, bytes_pp) = match mode.bpp {
            8 => (0u8, 18u32, 1u32),
            16 => (1u8, 19u32, 2u32),
            32 => (2u8, 20u32, 4u32),
            24 => (3u8, 20u32, 3u32),
            _ => return None,
        };
        if mode.pitch % bytes_pp != 0 {
            return None;
        }
        let pitch_px = mode.pitch / bytes_pp;
        let stride_hint: u8 = match mode.pitch {
            512 | 8192 => 0x00,
            1024 => 0x04,
            2048 => 0x08,
            4096 => 0x0C,
            _ => 0x00,
        };
        let oper = depth | stride_hint;
        let v1 = pitch_px.wrapping_shl(shift);
        Some((depth, shift, oper, v1))
    }

    /// Enable Trident New Mode, Graphics Engine (`3D4:36 |= 0x80`), and MMIO (`3D4:39 |= 0x01`)
    /// via VGA PIO before accessing BAR1 MMIO registers.
    pub fn enable_ge_and_mmio<P: VgaPio>(&mut self, pio: &mut P) {
        if self.mmio_enabled {
            return;
        }
        // 1. Enter New Mode: outb(0x3C4, 0x0B); inb(0x3C5)
        pio.outb(0x3C4, 0x0B);
        let _ = pio.inb(0x3C5);
        // 2. Unlock Protection (3C4:11 <- 0x92) and NewMode1 (3C4:0E <- 0x80)
        pio.outb(0x3C4, 0x11);
        pio.outb(0x3C5, 0x92);
        pio.outb(0x3C4, 0x0E);
        pio.outb(0x3C5, 0x80);
        // 3. Enable Graphics Engine (3D4:36 |= 0x80)
        pio.outb(0x3D4, 0x36);
        let ge = pio.inb(0x3D5);
        pio.outb(0x3D5, ge | 0x80);
        // 4. Enable MMIO (3D4:39 |= 0x01) — NOTE: VGA PIO is blocked while bit 0 is set!
        pio.outb(0x3D4, 0x39);
        let pci_reg = pio.inb(0x3D5);
        pio.outb(0x3D5, pci_reg | 0x01);
        self.mmio_enabled = true;
    }

    /// Wait for BladeXP GE idle (`(read8(0x2120) & 0x80) == 0`); reset GE on timeout.
    pub fn wait_idle<B: MmioBus>(&self, bus: &mut B) -> Result<(), u32> {
        let mut spins = 0u32;
        while (bus.read8(XP_REG_STATUS) & XP_STATUS_BUSY) != 0 {
            spins += 1;
            if spins >= XP_IDLE_TIMEOUT_ITERS {
                bus.write8(XP_REG_STATUS, 0x00);
                return Err(VBE_ERR_HW_TIMEOUT);
            }
        }
        Ok(())
    }
}

impl Accel2d for Trident2dEngine {
    #[inline]
    fn is_hw_ready(&self) -> bool {
        self.hw_ready
    }

    fn configure_mode<B: MmioBus, P: VgaPio>(
        &mut self,
        bus: &mut B,
        pio: &mut P,
        mode: &VbeModeDesc,
    ) -> Result<(), u32> {
        let Some((_depth, _shift, oper, v1)) = Self::compute_mode_params(mode) else {
            self.hw_ready = false;
            return Err(VBE_ERR_INVALID_MODE);
        };

        self.enable_ge_and_mmio(pio);
        self.wait_idle(bus)?;

        bus.write8(XP_REG_OPER, oper);
        self.eng_oper = oper | 0x40;
        bus.write32(XP_REG_DST_OFFSET, v1);
        bus.write32(XP_REG_SRC_OFFSET, v1);
        bus.write8(XP_REG_CLIP_CTL, 0x03);

        self.active_bpp = mode.bpp;
        self.active_pitch = mode.pitch;
        self.hw_ready = true;
        Ok(())
    }

    fn fill_rect<B: MmioBus>(
        &mut self,
        bus: &mut B,
        mode: &VbeModeDesc,
        req: &VbeFillReq,
    ) -> Result<(), u32> {
        if !self.hw_ready {
            return Err(VBE_ERR_UNSUPPORTED);
        }
        if req.rop != 0 && req.rop != VBE_ROP_PATCOPY && req.rop != VBE_ROP_SRCCOPY {
            return Err(VBE_ERR_UNSUPPORTED);
        }
        req.validate_bounds(mode)?;
        if req.w == 0 || req.h == 0 {
            return Ok(());
        }

        self.wait_idle(bus)?;
        let color = Self::replicate_color(req.color, mode.bpp);
        let dim = ((req.w as u32) << 16) | (req.h as u32);
        let dst = ((req.x as u32) << 16) | (req.y as u32);

        bus.write8(XP_REG_ROP, XP_ROP_PATCOPY);
        bus.write32(XP_REG_FG_COLOR, color);
        bus.write32(XP_REG_DRAW_MODE, XP_MODE_SOLIDFILL);
        bus.write32(XP_REG_DIM_WH, dim);
        bus.write32(XP_REG_DST_XY, dst);
        bus.write8(XP_REG_COMMAND, XP_CMD_GE_BLT);
        bus.write8(XP_REG_OPER, self.eng_oper);
        Ok(())
    }

    fn bit_blt<B: MmioBus>(
        &mut self,
        bus: &mut B,
        mode: &VbeModeDesc,
        req: &VbeBlitReq,
    ) -> Result<(), u32> {
        if !self.hw_ready {
            return Err(VBE_ERR_UNSUPPORTED);
        }
        if req.rop != 0 && req.rop != VBE_ROP_SRCCOPY {
            return Err(VBE_ERR_UNSUPPORTED);
        }
        req.validate_bounds(mode)?;
        if req.w == 0 || req.h == 0 || (req.src_x == req.dst_x && req.src_y == req.dst_y) {
            return Ok(());
        }

        let mut dir = XP_MODE_SCR2SCR;
        let mut sx = req.src_x as u32;
        let mut sy = req.src_y as u32;
        let mut dx = req.dst_x as u32;
        let mut dy = req.dst_y as u32;

        if sx < dx && sy == dy {
            dir |= XP_DIR_XNEG;
            sx = sx.checked_add((req.w - 1) as u32).ok_or(VBE_ERR_INVALID_RECT)?;
            dx = dx.checked_add((req.w - 1) as u32).ok_or(VBE_ERR_INVALID_RECT)?;
        }
        if sy < dy {
            dir |= XP_DIR_YNEG;
            sy = sy.checked_add((req.h - 1) as u32).ok_or(VBE_ERR_INVALID_RECT)?;
            dy = dy.checked_add((req.h - 1) as u32).ok_or(VBE_ERR_INVALID_RECT)?;
        }

        self.wait_idle(bus)?;
        bus.write32(XP_REG_DRAW_MODE, dir);
        bus.write8(XP_REG_ROP, XP_ROP_SRCCOPY);
        bus.write32(XP_REG_SRC_XY, (sx << 16) | sy);
        bus.write32(XP_REG_DST_XY, (dx << 16) | dy);
        bus.write32(XP_REG_DIM_WH, ((req.w as u32) << 16) | (req.h as u32));
        bus.write8(XP_REG_COMMAND, XP_CMD_GE_BLT);
        Ok(())
    }

    fn sync<B: MmioBus>(&mut self, bus: &mut B) -> Result<(), u32> {
        if !self.hw_ready {
            return Ok(());
        }
        self.wait_idle(bus)
    }

    /// Sync GE and clear `PCIReg` bit 0 (`0x3D4:0x39 &= ~0x01`) via MMIO so subsequent
    /// VGA PIO / BIOS `INT 10h` calls work cleanly.
    fn release_hw<B: MmioBus>(&mut self, bus: &mut B) -> Result<(), u32> {
        if !self.mmio_enabled {
            self.hw_ready = false;
            return Ok(());
        }
        let res = self.wait_idle(bus);
        bus.write8(XP_MMIO_CRTC_IDX, 0x39);
        let pci_reg = bus.read8(XP_MMIO_CRTC_DATA);
        bus.write8(XP_MMIO_CRTC_DATA, pci_reg & !0x01);
        self.mmio_enabled = false;
        self.hw_ready = false;
        res
    }
}

/// Scan PCI buses 0 and 1 (AGP) using a caller-supplied config-read closure.
pub fn probe_trident_pci_with<F>(mut read32: F) -> Option<TridentPciInfo>
where
    F: FnMut(u8, u8, u8, u8) -> u32,
{
    for bus in 0u8..=1 {
        for dev in 0u8..32 {
            let id = read32(bus, dev, 0, 0x00);
            let vendor = (id & 0xFFFF) as u16;
            let device = (id >> 16) as u16;
            if vendor != TRIDENT_PCI_VENDOR || device != TRIDENT_PCI_DEVICE_XP_AI1 {
                continue;
            }
            let class_rev = read32(bus, dev, 0, 0x08);
            let rev = (class_rev & 0xFF) as u8;
            let bar0 = read32(bus, dev, 0, 0x10);
            let bar1 = read32(bus, dev, 0, 0x14);
            // BAR0 (& 0xFFFFFFF0) = LFB; BAR1 (& 0xFFFFC000) = MMIO aperture.
            let lfb_bar0_phys = if (bar0 & 1) == 0 {
                bar0 & 0xFFFF_FFF0
            } else {
                0
            };
            let mmio_bar1_phys = if (bar1 & 1) == 0 {
                bar1 & 0xFFFF_C000
            } else {
                0
            };
            return Some(TridentPciInfo {
                bus,
                dev,
                func: 0,
                vendor,
                device,
                rev,
                lfb_bar0_phys,
                mmio_bar1_phys,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockBus {
        writes: Vec<(u32, u32, u8)>, // (offset, value, width_bytes)
        status: u8,
        crtc39: u8,
    }

    impl MmioBus for MockBus {
        fn read8(&self, offset: u32) -> u8 {
            match offset {
                XP_REG_STATUS => self.status,
                XP_MMIO_CRTC_DATA => self.crtc39,
                _ => 0,
            }
        }
        fn write8(&mut self, offset: u32, val: u8) {
            if offset == XP_MMIO_CRTC_DATA {
                self.crtc39 = val;
            }
            self.writes.push((offset, val as u32, 1));
        }
        fn read32(&self, _offset: u32) -> u32 {
            0
        }
        fn write32(&mut self, offset: u32, val: u32) {
            self.writes.push((offset, val, 4));
        }
    }

    #[derive(Default)]
    struct MockPio {
        ops: Vec<(bool, u16, u8)>, // (is_write, port, val)
    }

    impl VgaPio for MockPio {
        fn inb(&self, _port: u16) -> u8 {
            0x00
        }
        fn outb(&mut self, port: u16, val: u8) {
            self.ops.push((true, port, val));
        }
    }

    fn mode_1024x768x16() -> VbeModeDesc {
        VbeModeDesc {
            width: 1024,
            height: 768,
            pitch: 2048,
            bpp: 16,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            lfb_phys: 0xFC00_0000,
            lfb_size: 1024 * 768 * 2,
            mode_num: 0x4117,
            reserved: 0,
        }
    }

    #[test]
    fn pci_probe_finds_trident_8820_on_agp_bus1() {
        let info = probe_trident_pci_with(|bus, dev, _func, off| {
            if bus == 1 && dev == 0 {
                match off {
                    0x00 => {
                        ((TRIDENT_PCI_DEVICE_XP_AI1 as u32) << 16) | (TRIDENT_PCI_VENDOR as u32)
                    }
                    0x08 => 0x0300_0082,
                    0x10 => 0xFC00_0008,
                    0x14 => 0xE400_0000,
                    _ => 0,
                }
            } else {
                0xFFFF_FFFF
            }
        })
        .expect("should detect Trident 1023:8820 on bus 1");

        assert_eq!(info.bus, 1);
        assert_eq!(info.dev, 0);
        assert_eq!(info.vendor, 0x1023);
        assert_eq!(info.device, 0x8820);
        assert_eq!(info.rev, 0x82);
        assert_eq!(info.lfb_bar0_phys, 0xFC00_0000);
        assert_eq!(info.mmio_bar1_phys, 0xE400_0000);
    }

    #[test]
    fn xp_configure_fill_and_blit_emit_verified_register_sequences() {
        let mode = mode_1024x768x16();
        let mut engine = Trident2dEngine::new();
        let mut bus = MockBus {
            crtc39: 0x01,
            ..Default::default()
        };
        let mut pio = MockPio::default();

        assert_eq!(engine.configure_mode(&mut bus, &mut pio, &mode), Ok(()));
        assert!(engine.is_hw_ready());
        // 16bpp depth=1, stride_bytes=2048 -> hint=0x08 -> oper=0x09, eng_oper=0x49
        // pitch_px = 1024, shift = 19 -> v1 = 1024 << 19 = 0x2000_0000
        assert_eq!(engine.eng_oper, 0x49);
        assert_eq!(
            bus.writes,
            vec![
                (XP_REG_OPER, 0x09, 1),
                (XP_REG_DST_OFFSET, 1024 << 19, 4),
                (XP_REG_SRC_OFFSET, 1024 << 19, 4),
                (XP_REG_CLIP_CTL, 0x03, 1),
            ]
        );

        // Test Solid Fill (16bpp color 0xF800 replicated to 0xF800_F800)
        bus.writes.clear();
        let fill = VbeFillReq {
            x: 10,
            y: 20,
            w: 100,
            h: 50,
            color: 0xF800,
            rop: VBE_ROP_PATCOPY,
        };
        assert_eq!(engine.fill_rect(&mut bus, &mode, &fill), Ok(()));
        assert_eq!(
            bus.writes,
            vec![
                (XP_REG_ROP, 0xF0, 1),
                (XP_REG_FG_COLOR, 0xF800_F800, 4),
                (XP_REG_DRAW_MODE, XP_MODE_SOLIDFILL, 4),
                (XP_REG_DIM_WH, (100 << 16) | 50, 4),
                (XP_REG_DST_XY, (10 << 16) | 20, 4),
                (XP_REG_COMMAND, XP_CMD_GE_BLT as u32, 1),
                (XP_REG_OPER, 0x49, 1),
            ]
        );

        // Test overlapping BitBLT downward + rightward (YNEG, sy=10 < dy=20)
        bus.writes.clear();
        let blit = VbeBlitReq {
            src_x: 10,
            src_y: 10,
            dst_x: 10,
            dst_y: 20,
            w: 32,
            h: 16,
            rop: VBE_ROP_SRCCOPY,
        };
        assert_eq!(engine.bit_blt(&mut bus, &mode, &blit), Ok(()));
        assert_eq!(
            bus.writes,
            vec![
                (XP_REG_DRAW_MODE, XP_MODE_SCR2SCR | XP_DIR_YNEG, 4),
                (XP_REG_ROP, 0xCC, 1),
                (XP_REG_SRC_XY, (10 << 16) | 25, 4), // sy + h - 1 = 25
                (XP_REG_DST_XY, (10 << 16) | 35, 4), // dy + h - 1 = 35
                (XP_REG_DIM_WH, (32 << 16) | 16, 4),
                (XP_REG_COMMAND, XP_CMD_GE_BLT as u32, 1),
            ]
        );

        // Release HW clears PCIReg bit 0 via MMIO CRTC 0x3D4/0x3D5
        bus.writes.clear();
        assert_eq!(engine.release_hw(&mut bus), Ok(()));
        assert!(!engine.is_hw_ready());
        assert_eq!(bus.crtc39 & 0x01, 0);
    }
}
