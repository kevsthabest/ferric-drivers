//! Clean software Linear Framebuffer (LFB) 2D engine.
//!
//! Pure `#![no_std]` Rust, strictly 386-safe, zero panics on invalid input,
//! supporting 8bpp, 16bpp, 24bpp, and 32bpp packed-pixel formats and
//! overlap-safe screen-to-screen `BitBlt`.

use abi::vbe_abi::{
    VBE_ERR_INVALID_MODE, VBE_ERR_INVALID_RECT, VBE_OK, VbeBlitReq, VbeFillReq, VbeModeDesc,
};

/// Fill a rectangle `[x, x+w) x [y, y+h)` in `lfb` with a native-format `color`.
pub fn sw_fill_rect(lfb: &mut [u8], mode: &VbeModeDesc, req: &VbeFillReq) -> Result<(), u32> {
    let bytes_pp = mode.bytes_per_pixel().ok_or(VBE_ERR_INVALID_MODE)? as usize;
    req.validate_bounds(mode)?;
    if req.w == 0 || req.h == 0 {
        return Ok(());
    }
    let pitch = mode.pitch as usize;
    let x0 = req.x as usize;
    let y0 = req.y as usize;
    let w = req.w as usize;
    let h = req.h as usize;

    let last_row_end = (y0 + h - 1)
        .checked_mul(pitch)
        .and_then(|r| r.checked_add((x0 + w).checked_mul(bytes_pp)?))
        .ok_or(VBE_ERR_INVALID_RECT)?;
    if last_row_end > lfb.len() {
        return Err(VBE_ERR_INVALID_RECT);
    }

    let c0 = (req.color & 0xFF) as u8;
    let c1 = ((req.color >> 8) & 0xFF) as u8;
    let c2 = ((req.color >> 16) & 0xFF) as u8;
    let c3 = ((req.color >> 24) & 0xFF) as u8;

    let x_byte_off = x0 * bytes_pp;
    let row_bytes = w * bytes_pp;

    for row in 0..h {
        let start = (y0 + row) * pitch + x_byte_off;
        let dst = &mut lfb[start..start + row_bytes];
        match bytes_pp {
            1 => {
                for b in dst.iter_mut() {
                    *b = c0;
                }
            }
            2 => {
                let mut i = 0usize;
                while i + 2 <= dst.len() {
                    dst[i] = c0;
                    dst[i + 1] = c1;
                    i += 2;
                }
            }
            3 => {
                let mut i = 0usize;
                while i + 3 <= dst.len() {
                    dst[i] = c0;
                    dst[i + 1] = c1;
                    dst[i + 2] = c2;
                    i += 3;
                }
            }
            4 => {
                let mut i = 0usize;
                while i + 4 <= dst.len() {
                    dst[i] = c0;
                    dst[i + 1] = c1;
                    dst[i + 2] = c2;
                    dst[i + 3] = c3;
                    i += 4;
                }
            }
            _ => return Err(VBE_ERR_INVALID_MODE),
        }
    }

    Ok(())
}

/// Overlap-safe screen-to-screen BitBLT within `lfb`.
pub fn sw_bit_blt(lfb: &mut [u8], mode: &VbeModeDesc, req: &VbeBlitReq) -> Result<(), u32> {
    let bytes_pp = mode.bytes_per_pixel().ok_or(VBE_ERR_INVALID_MODE)? as usize;
    req.validate_bounds(mode)?;
    if req.w == 0 || req.h == 0 || (req.src_x == req.dst_x && req.src_y == req.dst_y) {
        return Ok(());
    }

    let pitch = mode.pitch as usize;
    let sx = req.src_x as usize;
    let sy = req.src_y as usize;
    let dx = req.dst_x as usize;
    let dy = req.dst_y as usize;
    let w = req.w as usize;
    let h = req.h as usize;

    let src_end = (sy + h - 1)
        .checked_mul(pitch)
        .and_then(|r| r.checked_add((sx + w).checked_mul(bytes_pp)?))
        .ok_or(VBE_ERR_INVALID_RECT)?;
    let dst_end = (dy + h - 1)
        .checked_mul(pitch)
        .and_then(|r| r.checked_add((dx + w).checked_mul(bytes_pp)?))
        .ok_or(VBE_ERR_INVALID_RECT)?;
    if src_end > lfb.len() || dst_end > lfb.len() {
        return Err(VBE_ERR_INVALID_RECT);
    }

    let row_bytes = w * bytes_pp;
    let sx_off = sx * bytes_pp;
    let dx_off = dx * bytes_pp;

    let copy_bottom_up = dy > sy;
    let copy_right_to_left = dy == sy && dx > sx;

    for step in 0..h {
        let row = if copy_bottom_up { h - 1 - step } else { step };
        let src_start = (sy + row) * pitch + sx_off;
        let dst_start = (dy + row) * pitch + dx_off;
        if copy_right_to_left {
            let mut i = row_bytes;
            while i > 0 {
                i -= 1;
                lfb[dst_start + i] = lfb[src_start + i];
            }
        } else {
            lfb.copy_within(src_start..src_start + row_bytes, dst_start);
        }
    }

    let _ = VBE_OK;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::vbe_abi::{VBE_ROP_PATCOPY, VBE_ROP_SRCCOPY};

    fn test_mode(w: u16, h: u16, bpp: u8) -> VbeModeDesc {
        let bpp_bytes = (bpp / 8) as u32;
        let pitch = (w as u32) * bpp_bytes;
        VbeModeDesc {
            width: w,
            height: h,
            pitch,
            bpp,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
            lfb_phys: 0xE000_0000,
            lfb_size: pitch * (h as u32),
            mode_num: 0x4112,
            reserved: 0,
        }
    }

    #[test]
    fn fill_rect_all_bpps() {
        for &bpp in &[8u8, 16, 24, 32] {
            let mode = test_mode(16, 16, bpp);
            let mut fb = vec![0u8; mode.lfb_size as usize];
            let color = 0x1122_3344u32;
            let req = VbeFillReq {
                x: 2,
                y: 3,
                w: 4,
                h: 5,
                color,
                rop: VBE_ROP_PATCOPY,
            };
            assert_eq!(sw_fill_rect(&mut fb, &mode, &req), Ok(()));
            let bpp_bytes = (bpp / 8) as usize;
            let pitch = mode.pitch as usize;
            for y in 0..16usize {
                for x in 0..16usize {
                    let off = y * pitch + x * bpp_bytes;
                    let inside = (2..6).contains(&x) && (3..8).contains(&y);
                    for b in 0..bpp_bytes {
                        let expected = if inside {
                            ((color >> (b * 8)) & 0xFF) as u8
                        } else {
                            0
                        };
                        assert_eq!(fb[off + b], expected, "bpp={bpp} x={x} y={y} b={b}");
                    }
                }
            }
        }
    }

    #[test]
    fn bit_blt_overlapping_all_directions() {
        let mode = test_mode(16, 16, 32);
        let pitch = mode.pitch as usize;

        // Fill a 4x4 pattern at (4, 4) with unique pixel values.
        let make_fb = || {
            let mut fb = vec![0u8; mode.lfb_size as usize];
            for y in 4..8usize {
                for x in 4..8usize {
                    let off = y * pitch + x * 4;
                    let val = ((y as u32) << 8) | (x as u32);
                    fb[off..off + 4].copy_from_slice(&val.to_le_bytes());
                }
            }
            fb
        };

        // Test all 4 overlapping shifts: right, left, down, up, plus diagonal down-right.
        for &(dx, dy) in &[(6u16, 4u16), (2, 4), (4, 6), (4, 2), (6, 6)] {
            let mut fb = make_fb();
            let req = VbeBlitReq {
                src_x: 4,
                src_y: 4,
                dst_x: dx,
                dst_y: dy,
                w: 4,
                h: 4,
                rop: VBE_ROP_SRCCOPY,
            };
            assert_eq!(sw_bit_blt(&mut fb, &mode, &req), Ok(()));
            for ry in 0..4usize {
                for rx in 0..4usize {
                    let off = (dy as usize + ry) * pitch + (dx as usize + rx) * 4;
                    let got = u32::from_le_bytes(fb[off..off + 4].try_into().unwrap());
                    let expected = (((4 + ry) as u32) << 8) | ((4 + rx) as u32);
                    assert_eq!(got, expected, "shift=({dx},{dy}) rx={rx} ry={ry}");
                }
            }
        }
    }

    #[test]
    fn rejects_out_of_bounds_and_allows_empty_rects() {
        let mode = test_mode(8, 8, 16);
        let mut fb = vec![0u8; mode.lfb_size as usize];
        let empty = VbeFillReq {
            x: 8,
            y: 8,
            w: 0,
            h: 0,
            color: 0xFFFF,
            rop: 0,
        };
        assert_eq!(sw_fill_rect(&mut fb, &mode, &empty), Ok(()));

        let oob = VbeFillReq {
            x: 7,
            y: 0,
            w: 2,
            h: 1,
            color: 0xFFFF,
            rop: 0,
        };
        assert_eq!(sw_fill_rect(&mut fb, &mode, &oob), Err(VBE_ERR_INVALID_RECT));

        let oob_blit = VbeBlitReq {
            src_x: 0,
            src_y: 0,
            dst_x: 7,
            dst_y: 0,
            w: 2,
            h: 1,
            rop: 0,
        };
        assert_eq!(
            sw_bit_blt(&mut fb, &mode, &oob_blit),
            Err(VBE_ERR_INVALID_RECT)
        );
    }
}
