//! Standalone Read-Only NTFS JLM (`NTFS.JLM`, DDB device ID `0xFD03`).
//!
//! Portable Stage F filesystem module: has ZERO dependency on `gate1-svc` or
//! OHCI hardware. All block I/O flows through the versioned `abi::fs_abi`
//! Ring-0 DDB service table exported by `FERRIC.JLM` (`0xFD01`) via JEMM's
//! native `INT 20h` dynamic-link mechanism.

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod ntfs_ro;

use abi::fs_abi::{
    BLK_FLAG_EJECTED, BLK_FLAG_READY, BlkDevInfo, blk_abi_compatible,
};
use core::cell::UnsafeCell;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// Ring-0 JEMM INT 20h dynamic-linked imports provided by asm/ntfs_jlm.asm
// (pointing at FERRIC.JLM 0xFD01 DDB service table indices 0, 1, 2).
#[cfg(not(test))]
unsafe extern "C" {
    fn blk_abi_version() -> u32;
    fn blk_dev_info(out_lin: u32) -> u32;
    fn blk_read_sec(lba: u32, dst_lin: u32) -> u32;
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

struct NtfsScratch {
    rec: [u8; ntfs_ro::MAX_RECORD_SIZE as usize],
    indx: [u8; ntfs_ro::MAX_RECORD_SIZE as usize],
}

impl NtfsScratch {
    const fn new() -> Self {
        Self {
            rec: [0u8; ntfs_ro::MAX_RECORD_SIZE as usize],
            indx: [0u8; ntfs_ro::MAX_RECORD_SIZE as usize],
        }
    }
}

struct SyncScratch(UnsafeCell<NtfsScratch>);
unsafe impl Sync for SyncScratch {}

static NTFS_SCRATCH: SyncScratch = SyncScratch(UnsafeCell::new(NtfsScratch::new()));

#[inline]
fn scratch() -> &'static mut NtfsScratch {
    unsafe { &mut *NTFS_SCRATCH.0.get() }
}

struct NtfsModState {
    mounted_gen: u16,
    vol: Option<ntfs_ro::NtfsVolume>,
    file: Option<ntfs_ro::NtfsEntry>,
    file_pos: u64,
}

impl NtfsModState {
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

struct SyncModState(UnsafeCell<NtfsModState>);
unsafe impl Sync for SyncModState {}

static NTFS_STATE: SyncModState = SyncModState(UnsafeCell::new(NtfsModState::new()));

#[inline]
fn state() -> &'static mut NtfsModState {
    unsafe { &mut *NTFS_STATE.0.get() }
}

fn core_read_sector(lba: u32, out: &mut [u8; 512]) -> bool {
    unsafe { blk_read_sec(lba, out.as_mut_ptr() as u32) != 0 }
}

/// Ensure the cached NTFS volume matches the core transport's live `media_gen`.
/// Re-mounts lazily on media insertion or swap; clears on eject or removal.
fn ensure_mounted() -> Option<ntfs_ro::NtfsVolume> {
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
    let sc = scratch();
    let vol = ntfs_ro::mount_volume(&mut sc.rec, core_read_sector);
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

fn push_utf16_ascii(w: &mut AsciiOut, name: &[u16]) {
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
/// Returns `1` on compatible core, `0` to abort module load cleanly.
#[unsafe(no_mangle)]
pub extern "C" fn ntfs_jlm_init() -> u32 {
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

/// V86 API `AX=0x0032` (`NTFS.COM` with no args): format NTFS volume summary +
/// root directory listing into `dst_lin[..cap]`. Returns bytes written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_jlm_dir(dst_lin: u32, cap: u32) -> u32 {
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
        w.push_bytes(b"NTFS: NO-NTFS\r\n");
        return w.pos as u32;
    };
    w.push_bytes(b"NTFS: label \"");
    push_utf16_ascii(&mut w, &vol.label[..vol.label_len as usize]);
    w.push_bytes(b"\" serial ");
    w.push_u64(vol.serial);
    let sc = scratch();
    if let Some(free) = ntfs_ro::free_clusters(&vol, &mut sc.rec, core_read_sector) {
        w.push_bytes(b" free ");
        w.push_u64(free);
        w.push_bytes(b" clusters");
    }
    w.push_bytes(b"\r\n");
    ntfs_ro::for_each_dir_entry(
        &vol,
        ntfs_ro::ROOT_RECORD,
        &mut sc.rec,
        &mut sc.indx,
        core_read_sector,
        |entry| {
            w.push_bytes(b"  ");
            push_utf16_ascii(&mut w, &entry.name[..entry.name_len as usize]);
            if entry.is_dir {
                w.push_bytes(b" <DIR>\r\n");
            } else {
                w.push_bytes(b" ");
                w.push_u64(entry.data_len);
                w.push_bytes(b" B\r\n");
            }
            w.pos + 32 < w.buf.len()
        },
    );
    w.pos as u32
}

/// V86 API `AX=0x0033` (`NTFS.COM <path>`): open NUL-terminated ASCII path at
/// `name_lin` on the NTFS volume. Returns `1` if found, `0` otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_jlm_open(name_lin: u32) -> u32 {
    if !is_valid_dos_buf(name_lin, 256) {
        return 0;
    }
    let Some(vol) = ensure_mounted() else {
        return 0;
    };
    let mut path_utf16 = [0u16; 256];
    let mut len = 0usize;
    let src = name_lin as *const u8;
    while len < path_utf16.len() {
        let b = unsafe { core::ptr::read_volatile(src.add(len)) };
        if b == 0 {
            break;
        }
        path_utf16[len] = b as u16;
        len += 1;
    }
    if len == 0 {
        return 0;
    }
    let sc = scratch();
    let entry = ntfs_ro::find_path(
        &vol,
        &path_utf16[..len],
        &mut sc.rec,
        &mut sc.indx,
        core_read_sector,
    );
    let st = state();
    match entry {
        Some(e) if !e.is_dir => {
            st.file = Some(e);
            st.file_pos = 0;
            1
        }
        _ => {
            st.file = None;
            st.file_pos = 0;
            0
        }
    }
}

/// V86 API `AX=0x0034` (`NTFS.COM`): read next up-to-512-byte chunk of the
/// opened NTFS file into `dst_lin`. Returns bytes read (`0` = EOF, `0xFFFF` = error).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ntfs_jlm_read_chunk(dst_lin: u32) -> u32 {
    if !is_valid_dos_buf(dst_lin, 512) {
        return 0xFFFF;
    }
    let Some(vol) = ensure_mounted() else {
        return 0xFFFF;
    };
    let (entry, pos) = {
        let st = state();
        let Some(e) = st.file else {
            return 0xFFFF;
        };
        (e, st.file_pos)
    };
    let mut buf = [0u8; 512];
    let sc = scratch();
    let n = ntfs_ro::read_entry_data(&vol, &entry, pos, &mut buf, &mut sc.rec, core_read_sector);
    if n == 0 {
        state().file = None;
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst_lin as *mut u8, n);
    }
    state().file_pos = pos.saturating_add(n as u64);
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntfs_jlm_init_accepts_compatible_core_abi() {
        assert_eq!(ntfs_jlm_init(), 1);
    }
}
