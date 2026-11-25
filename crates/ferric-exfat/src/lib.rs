//! Standalone Read/Write exFAT + DOS INT 2Fh Redirector JLM (`EXFAT.JLM`, DDB `0xFD02`).
//!
//! Portable Stage F filesystem module: has ZERO dependency on `gate1-svc` or
//! OHCI hardware. All block I/O flows through the versioned `abi::fs_abi`
//! Ring-0 DDB service table exported by `FERRIC.JLM` (`0xFD01`) via JEMM's
//! native `INT 20h` dynamic-link mechanism.

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod exfat_ro;
pub mod exfat_rw;
pub mod redir;

use abi::fs_abi::{
    BLK_FLAG_EJECTED, BLK_FLAG_READY, BlkDevInfo, blk_abi_compatible,
};
use core::cell::UnsafeCell;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// Ring-0 JEMM INT 20h dynamic-linked imports provided by asm/exfat_jlm.asm
// (pointing at FERRIC.JLM 0xFD01 DDB service table indices 0..4).
#[cfg(not(test))]
unsafe extern "C" {
    fn blk_abi_version() -> u32;
    fn blk_dev_info(out_lin: u32) -> u32;
    fn blk_read_sec(lba: u32, dst_lin: u32) -> u32;
    fn blk_write_sec(lba: u32, src_lin: u32) -> u32;
    fn blk_flush() -> u32;
}

#[cfg(test)]
unsafe fn blk_abi_version() -> u32 {
    abi::fs_abi::FERRIC_BLK_ABI_VERSION
}

#[cfg(test)]
unsafe fn blk_dev_info(_out_lin: u32) -> u32 {
    0
}

#[cfg(test)]
unsafe fn blk_read_sec(_lba: u32, _dst_lin: u32) -> u32 {
    0
}

#[cfg(test)]
unsafe fn blk_write_sec(_lba: u32, _src_lin: u32) -> u32 {
    0
}

#[cfg(test)]
unsafe fn blk_flush() -> u32 {
    0
}

struct ExfatModState {
    mounted_gen: u16,
    vol: Option<exfat_ro::ExfatVolume>,
    file: Option<exfat_ro::ExfatEntry>,
    file_pos: u64,
}

impl ExfatModState {
    const fn new() -> Self {
        Self {
            mounted_gen: 0,
            vol: None,
            file: None,
            file_pos: 0,
        }
    }

    fn clear(&mut self) {
        self.mounted_gen = 0;
        self.vol = None;
        self.file = None;
        self.file_pos = 0;
    }
}

struct SyncModState(UnsafeCell<ExfatModState>);
unsafe impl Sync for SyncModState {}

static EXFAT_STATE: SyncModState = SyncModState(UnsafeCell::new(ExfatModState::new()));

#[inline]
fn state() -> &'static mut ExfatModState {
    unsafe { &mut *EXFAT_STATE.0.get() }
}

/// Thin block-transport shim matching the 4-function surface `redir.rs` expects,
/// backed entirely by `INT 20h` DDB calls into `FERRIC.JLM` (`0xFD01`).
pub(crate) mod ohci {
    use super::*;

    pub(crate) unsafe fn msc_read_sector(lba: u32, out: &mut [u8; 512]) -> bool {
        unsafe { blk_read_sec(lba, out.as_mut_ptr() as u32) != 0 }
    }

    pub(crate) unsafe fn msc_write_sector(lba: u32, data: &[u8; 512]) -> bool {
        unsafe { blk_write_sec(lba, data.as_ptr() as u32) != 0 }
    }

    pub(crate) unsafe fn msc_sync_cache() -> bool {
        unsafe { blk_flush() != 0 }
    }

    pub(crate) fn redir_xvol() -> Option<exfat_ro::ExfatVolume> {
        ensure_mounted()
    }
}

/// Ensure the cached exFAT volume matches the core transport's live `media_gen`.
/// Re-mounts lazily on media insertion or swap; clears on eject or removal.
fn ensure_mounted() -> Option<exfat_ro::ExfatVolume> {
    let mut info = BlkDevInfo::empty();
    let ok = unsafe { blk_dev_info(&mut info as *mut BlkDevInfo as u32) };
    if ok == 0
        || (info.flags & BLK_FLAG_READY) == 0
        || (info.flags & BLK_FLAG_EJECTED) != 0
    {
        state().clear();
        return None;
    }
    let st = state();
    if st.mounted_gen == info.media_gen && st.vol.is_some() {
        return st.vol;
    }
    st.clear();
    let vol = exfat_ro::mount_volume(|lba, out| unsafe { ohci::msc_read_sector(lba, out) });
    st.vol = vol;
    st.mounted_gen = info.media_gen;
    vol
}

/// Tiny ASCII formatter writing into a caller-provided DOS buffer.
struct AsciiOut<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> AsciiOut<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn push_byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }

    fn push_bytes(&mut self, s: &[u8]) {
        for &b in s {
            self.push_byte(b);
        }
    }

    fn push_u32(&mut self, mut v: u32) {
        if v == 0 {
            self.push_byte(b'0');
            return;
        }
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push_byte(tmp[n]);
        }
    }

    fn push_u64(&mut self, mut v: u64) {
        if v == 0 {
            self.push_byte(b'0');
            return;
        }
        let mut tmp = [0u8; 20];
        let mut n = 0usize;
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push_byte(tmp[n]);
        }
    }
}

fn push_exfat_name(w: &mut AsciiOut, name: &[u16]) {
    for &ch in name {
        if (0x20..0x7F).contains(&ch) {
            w.push_byte(ch as u8);
        } else {
            w.push_byte(b'?');
        }
    }
}

/// Called from `DllMain` (`DLL_PROCESS_ATTACH`): verify core block transport
/// `0xFD01` exposes a compatible `abi::fs_abi` version (>= 1.0, same major).
#[unsafe(no_mangle)]
pub extern "C" fn exfat_jlm_init() -> u32 {
    state().clear();
    let ver = unsafe { blk_abi_version() };
    if blk_abi_compatible(ver, 1, 0) { 1 } else { 0 }
}

/// Reject pointers into IVT/BDA (`< 0x0500`) or above conventional memory (`> 0xA0000`).
#[inline]
const fn is_valid_dos_buf(lin: u32, len: u32) -> bool {
    if lin < 0x0500 || len == 0 {
        return false;
    }
    match lin.checked_add(len) {
        Some(end) => end <= 0x000A_0000,
        None => false,
    }
}

/// V86 API `AX=0x001B` (`EXFAT.COM` with no args): format exFAT volume summary +
/// root directory listing into `dst_lin[..cap]`. Returns bytes written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exfat_jlm_dir(dst_lin: u32, cap: u32) -> u32 {
    if !is_valid_dos_buf(dst_lin, cap) {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(dst_lin as *mut u8, cap as usize) };
    let mut w = AsciiOut::new(slice);
    let mut info = BlkDevInfo::empty();
    if unsafe { blk_dev_info(&mut info as *mut BlkDevInfo as u32) } == 0
        || (info.flags & BLK_FLAG_READY) == 0
        || (info.flags & BLK_FLAG_EJECTED) != 0
    {
        return 0;
    }
    let Some(vol) = ensure_mounted() else {
        w.push_bytes(b"EXFAT: NO-EXFAT\r\n");
        return w.pos as u32;
    };
    w.push_bytes(b"EXFAT: label \"");
    push_exfat_name(&mut w, &vol.label[..vol.label_len as usize]);
    w.push_bytes(b"\" serial ");
    w.push_u32(vol.serial);
    if let Some(free) = exfat_ro::free_clusters(&vol, |lba, out| unsafe {
        ohci::msc_read_sector(lba, out)
    }) {
        w.push_bytes(b" free ");
        w.push_u32(free);
        w.push_bytes(b" clusters");
    }
    w.push_bytes(b"\r\n");
    exfat_ro::for_each_dir_entry(
        &vol,
        vol.root_cluster,
        |lba, out| unsafe { ohci::msc_read_sector(lba, out) },
        |e| {
            w.push_bytes(b"  ");
            push_exfat_name(&mut w, &e.name[..e.name_len as usize]);
            if e.is_dir {
                w.push_bytes(b" <DIR>\r\n");
            } else {
                w.push_bytes(b" ");
                w.push_u64(e.data_len);
                w.push_bytes(b" B\r\n");
            }
            w.pos + 32 < w.buf.len()
        },
    );
    w.pos as u32
}

/// V86 API `AX=0x001C` (`EXFAT.COM <path>`): open NUL-terminated ASCII path at
/// `name_lin` on the exFAT volume. Returns `1` if found, `0` otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exfat_jlm_open(name_lin: u32) -> u32 {
    if !is_valid_dos_buf(name_lin, 128) {
        return 0;
    }
    let Some(vol) = ensure_mounted() else {
        return 0;
    };
    let ptr = name_lin as *const u8;
    let mut len = 0usize;
    while len < 128 {
        if unsafe { core::ptr::read_volatile(ptr.add(len)) } == 0 {
            break;
        }
        len += 1;
    }
    if len == 0 {
        return 0;
    }
    let mut path = [0u16; 128];
    for i in 0..len {
        path[i] = unsafe { core::ptr::read_volatile(ptr.add(i)) } as u16;
    }
    let Some(ent) = exfat_ro::find_path(
        &vol,
        &path[..len],
        |lba, out| unsafe { ohci::msc_read_sector(lba, out) },
    ) else {
        return 0;
    };
    if ent.is_dir {
        return 0;
    }
    let st = state();
    st.file = Some(ent);
    st.file_pos = 0;
    1
}

/// V86 API `AX=0x001D`: read next up-to-512-byte chunk of the opened exFAT file
/// into `dst_lin`. Returns bytes read (`0` = EOF, `0xFFFF` = error).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exfat_jlm_read_chunk(dst_lin: u32) -> u32 {
    if !is_valid_dos_buf(dst_lin, 512) {
        return 0xFFFF;
    }
    let Some(vol) = ensure_mounted() else {
        return 0xFFFF;
    };
    let (ent, pos) = {
        let st = state();
        let Some(e) = st.file else {
            return 0xFFFF;
        };
        (e, st.file_pos)
    };
    let mut sec = [0u8; 512];
    let n = exfat_ro::read_entry_data(
        &vol,
        &ent,
        pos,
        &mut sec,
        |lba, out| unsafe { ohci::msc_read_sector(lba, out) },
    );
    if n == 0 {
        state().file = None;
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(sec.as_ptr(), dst_lin as *mut u8, n);
    }
    state().file_pos = pos.saturating_add(n as u64);
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exfat_jlm_init_accepts_compatible_core_abi() {
        assert_eq!(exfat_jlm_init(), 1);
    }
}
