//! Stage C/D: the INT 2Fh redirector core for the exFAT volume.
//!
//! XREDIR.COM owns the DOS side of the interface: it claims a CDS entry,
//! hooks INT 2Fh, and marshals the kernel's registers and pointers into
//! a fixed request block in low memory. This module owns everything
//! behind that block — the open-file table, the find-first/next scans,
//! and every answer the kernel gets. The split exists because the TSR
//! must stay tiny and dumb (it runs on the kernel's stack, in V86 mode),
//! while the filesystem knowledge already lives here next to the exFAT
//! parser and the USB mass-storage state.
//!
//! The register and memory conventions implemented here are pinned
//! against the vendored FreeDOS kernel sources, not against generic
//! redirector documentation: the SDA field offsets come from
//! `kernel/kernel.asm` (`_internal_data`), the SFT layout from
//! `hdr/sft.h`, the per-subfunction register rules from
//! `kernel/int2f.asm` (which zeroes CX for the generic subfunctions and
//! passes results back in CX), and the close/count ownership from
//! `kernel/dosfns.c` (the kernel bumps a zero `sft_count` to one after
//! a successful remote open and expects the redirector to subtract it
//! again on close — see the "remote sub sft_count" comment there).
//!
//! Stage D adds the write family on top of the Stage C read shell:
//! files can be created, written, truncated (via create/replace), and
//! deleted, through the writer core in `exfat_rw`, which shares the
//! parser's view of the volume but none of its code paths beyond the
//! checksum helpers. Directory mutation stays out of scope for this
//! stage: MKDIR, RMDIR, RENAME, and SETATTR are still refused with DOS
//! error 5 (access denied), the answer DOS itself gives for an
//! operation the medium does not support.

use core::cell::UnsafeCell;

use crate::exfat_ro::{self, ExfatEntry, ExfatVolume};
use crate::exfat_rw::{self, Alloc, SetLoc};
use crate::ohci;

// ---------------------------------------------------------------------------
// DOS error codes (the values the kernel negates into its internal form).
// ---------------------------------------------------------------------------

const ERR_INVALID_FUNCTION: u16 = 1;
const ERR_FILE_NOT_FOUND: u16 = 2;
const ERR_PATH_NOT_FOUND: u16 = 3;
const ERR_TOO_MANY_OPEN: u16 = 4;
const ERR_ACCESS_DENIED: u16 = 5;
const ERR_INVALID_HANDLE: u16 = 6;
const ERR_NO_MORE_FILES: u16 = 18;
const ERR_WRITE_FAULT: u16 = 29;

// ---------------------------------------------------------------------------
// Request block layout, shared with XREDIR.COM. The TSR fills the input
// fields from the INT 2Fh entry state; this module writes the outputs.
// ---------------------------------------------------------------------------

const REQ_FN: usize = 0x00; // in/out: full AX (0x11xx), then result AX
const REQ_BX: usize = 0x02; // in/out
const REQ_CX: usize = 0x04; // in/out
const REQ_DX: usize = 0x06; // in/out
const REQ_DI: usize = 0x0A; // in
const REQ_DS: usize = 0x0C; // in (the exiting PSP on PROCESS_END)
const REQ_ES: usize = 0x0E; // in
const REQ_OUT_FLAGS: usize = 0x10; // out: bit 0 = carry, bit 1 = consumed
const REQ_SDA_SEG: usize = 0x12; // in: SDA far pointer (INT 21h AX=5D06h)
const REQ_SDA_OFF: usize = 0x14;
// 0x16..0x19 carry this drive's CDS far pointer. No handler reads it:
// ownership of CDS-addressed calls is tested through the CDS's own
// contents, which does not depend on pointer normalisation.
const REQ_DRIVE: usize = 0x1A; // in: drive number, 0 = A:

const OUT_FLAG_CARRY: u16 = 0x0001;
const OUT_FLAG_CONSUMED: u16 = 0x0002;

// ---------------------------------------------------------------------------
// SDA field offsets from _internal_data (kernel/kernel.asm).
// ---------------------------------------------------------------------------

const SDA_DTA: usize = 0x00C; // far pointer to the current DTA
const SDA_PRI_PATH: usize = 0x09E; // fully qualified path of the current op
const SDA_TMP_DM: usize = 0x19E; // 21-byte find state (dmatch)
const SDA_SEARCH_DIR: usize = 0x1B3; // 32-byte dirent the kernel returns
const SDA_DIR_ENT_BUF: usize = 0x22B; // DirEntBuffer: FCB name of last component
const SDA_SATTR: usize = 0x24D; // attribute mask for the current search
const SDA_OPEN_MODE: usize = 0x24E; // low byte of the open flags
const SDA_EXT_ACTION: usize = 0x2DD; // extended-open action code
const SDA_EXT_ATTRIB: usize = 0x2DF; // extended-open create attributes
const SDA_EXT_MODE: usize = 0x2E1; // extended-open mode (access in low bits)

// ---------------------------------------------------------------------------
// SFT field offsets (hdr/sft.h, WITHFAT32 layout).
// ---------------------------------------------------------------------------

const SFT_COUNT: usize = 0x00; // reference count (redirector subtracts on close)
const SFT_ATTRIB: usize = 0x04;
const SFT_FLAGS: usize = 0x05;
const SFT_TIME: usize = 0x0D;
const SFT_DATE: usize = 0x0F;
const SFT_SIZE: usize = 0x11;
const SFT_POSIT: usize = 0x15;
const SFT_NAME: usize = 0x20; // 11-byte FCB-format name
const SFT_PSP: usize = 0x31;

const SFT_FSHARED: u16 = 0x8000; // kernel routes SFT ops to the redirector
const SFT_FCLEAN: u16 = 0x0040; // file has not been written to

/// Timestamp stamped on files this stage creates: 1980-01-01 00:00:00,
/// the DOS epoch, in exFAT's packing (day at bit 16, month at bit 21).
/// The JLM has no clock source of its own; an honest epoch stamp beats
/// an invented plausible-looking time.
const CREATE_TIMESTAMP: u32 = 0x0021_0000;

// Attribute bits (hdr/fs.h D_* values, same positions as exFAT's).
const ATTR_READONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLID: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_DEVICE: u8 = 0x40;

// dmatch (find state) field offsets inside the 21-byte record. The layout
// is the kernel's own struct dmatch (hdr/dirmatch.h); only the fields
// below carry meaning across a find-first/find-next pair for us.
const DM_DRIVE: usize = 0x00; // 0xC0 | drive: bits mark "redirector search"
const DM_NAME_PAT: usize = 0x01; // 11-byte FCB pattern with '?' wildcards
const DM_ATTR_SRCH: usize = 0x0C;
const DM_ENTRY: usize = 0x0D; // u16: raw entries already examined
const DM_DIRCLUSTER: usize = 0x0F; // u32: first cluster of the scanned dir
const DM_STAGE: usize = 0x13; // ours: 1 once the volume label was emitted

// ---------------------------------------------------------------------------
// Low-memory access. Everything the kernel points at lives below 1 MiB;
// the JLM runs with that memory mapped, as the existing exFAT verbs do.
// ---------------------------------------------------------------------------

fn far_lin(seg: u16, off: u16) -> u32 {
    ((seg as u32) << 4) + off as u32
}

unsafe fn rd8(lin: u32) -> u8 {
    unsafe { core::ptr::read_volatile(lin as *const u8) }
}
unsafe fn rd16(lin: u32) -> u16 {
    unsafe { core::ptr::read_volatile(lin as *const u16) }
}
unsafe fn rd32(lin: u32) -> u32 {
    unsafe { core::ptr::read_volatile(lin as *const u32) }
}
unsafe fn wr8(lin: u32, v: u8) {
    unsafe { core::ptr::write_volatile(lin as *mut u8, v) }
}
unsafe fn wr16(lin: u32, v: u16) {
    unsafe { core::ptr::write_volatile(lin as *mut u16, v) }
}
unsafe fn wr32(lin: u32, v: u32) {
    unsafe { core::ptr::write_volatile(lin as *mut u32, v) }
}
unsafe fn copy_to_v86(dst_lin: u32, src: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst_lin as *mut u8, src.len());
    }
}
unsafe fn copy_from_v86(dst: &mut [u8], src_lin: u32) {
    unsafe {
        core::ptr::copy_nonoverlapping(src_lin as *const u8, dst.as_mut_ptr(), dst.len());
    }
}

// ---------------------------------------------------------------------------
// Open-file table. Keyed by the SFT far pointer because this kernel's
// SFT has no IFS-pointer field: the address itself is the only handle
// that survives between the kernel's calls.
// ---------------------------------------------------------------------------

const MAX_OPEN: usize = 16;

#[derive(Clone, Copy)]
struct OpenRec {
    used: bool,
    sft_lin: u32,
    entry: ExfatEntry,
    // Chain cursor: the cluster that contains file offset `cursor_pos`,
    // and its index in the chain. Reads move it forward one cluster at a
    // time, so a sequential read never re-walks the chain from the head
    // (the quadratic behaviour the Stage B copier accepted and this
    // stage exists to remove).
    cursor_cluster: u32,
    cursor_index: u32,
    cursor_pos: u64,
    psp: u16,
    // Stage D write state. `access` is the open mode's access bits
    // (0 = read, 1 = write, 2 = read/write); a write through a record
    // with access 0 is refused, the same answer DOS gives for a write
    // on a read-only handle. `alloc` is the live allocation this
    // record's writes grow, and `loc` says where the file's entry set
    // lives so its stream fields can be patched at persist time.
    // `loc` is None only for files the parser found but the writer
    // cannot address (long names DOS cannot carry); those opens are
    // forced read-only, because there is no set to patch. `dirty`
    // marks that the on-disk stream entry no longer describes this
    // record's entry/alloc pair and must be rewritten at close/flush.
    access: u8,
    dirty: bool,
    loc: Option<SetLoc>,
    alloc: Alloc,
}

impl OpenRec {
    const fn empty() -> Self {
        OpenRec {
            used: false,
            sft_lin: 0,
            entry: ExfatEntry {
                name: [0; 128],
                name_len: 0,
                name_truncated: false,
                attr: 0,
                is_dir: false,
                first_cluster: 0,
                data_len: 0,
                valid_len: 0,
                no_fat_chain: false,
                modified: 0,
            },
            cursor_cluster: 0,
            cursor_index: 0,
            cursor_pos: 0,
            psp: 0,
            access: 0,
            dirty: false,
            loc: None,
            alloc: Alloc::EMPTY,
        }
    }
}

struct RedirState {
    opens: [OpenRec; MAX_OPEN],
    // Serial of the volume the open table belongs to. Media can be
    // ejected and swapped under a live redirector; SFT pointers would
    // silently alias the new medium's files, so any serial change drops
    // every open record before the next call is answered.
    vol_serial: u32,
    // True while this session has the volume's dirty flag set on the
    // medium. Tracked here so the flag is written once at the first
    // write of a session and cleared once, when the last dirty record
    // has been persisted — not flipped per operation.
    vol_dirty: bool,
}

struct RedirCell(UnsafeCell<RedirState>);
unsafe impl Sync for RedirCell {}

static REDIR: RedirCell = RedirCell(UnsafeCell::new(RedirState {
    opens: [OpenRec::empty(); MAX_OPEN],
    vol_serial: 0,
    vol_dirty: false,
}));

fn redir_state() -> &'static mut RedirState {
    unsafe { &mut *REDIR.0.get() }
}

// ---------------------------------------------------------------------------
// Pure helpers (host-tested): DOS name forms, pattern matching, paths.
// ---------------------------------------------------------------------------

/// Convert an exFAT name to the 11-byte space-padded FCB form DOS uses,
/// or `None` when the name cannot be represented in 8.3 at all. DOS has
/// no alias mechanism in this design: an unrepresentable name is simply
/// unreachable through the drive letter (it stays reachable through
/// EXFAT.COM, which speaks long names), and find operations skip it
/// rather than inventing a name the user never saw on the medium.
pub fn dos_name_83(name: &[u16]) -> Option<[u8; 11]> {
    let mut out = [b' '; 11];
    let mut base = 0usize; // chars stored in the base field
    let mut ext = 0usize; // chars stored in the extension field
    let mut in_ext = false;
    for &ch in name {
        if ch == b'.' as u16 {
            if in_ext || base == 0 {
                return None; // second dot, or a leading dot: not 8.3
            }
            in_ext = true;
            continue;
        }
        if !(0x21..=0x7E).contains(&ch) {
            return None; // space, control, DEL, or non-ASCII
        }
        let c = ch as u8;
        // Characters DOS forbids in filenames (the set truename's
        // DirChar rejects on the way in, mirrored on the way out).
        if matches!(
            c,
            b'"' | b'*' | b'+' | b',' | b'/' | b':' | b';' | b'<' | b'=' | b'>' | b'?' | b'['
                | b'\\' | b']' | b'|'
        ) {
            return None;
        }
        let up = if c.is_ascii_lowercase() {
            c - b'a' + b'A'
        } else {
            c
        };
        if in_ext {
            if ext >= 3 {
                return None;
            }
            out[8 + ext] = up;
            ext += 1;
        } else {
            if base >= 8 {
                return None;
            }
            out[base] = up;
            base += 1;
        }
    }
    if base == 0 {
        return None;
    }
    Some(out)
}

/// The kernel's own wildcard rule (`fcmp_wild` in kernel/fatfs.c): a
/// '?' in the pattern matches any single byte, everything else must be
/// byte-equal. '*' never reaches this function — truename has already
/// expanded it into a run of '?' inside PriPathName.
pub fn fcb_match(pat: &[u8; 11], name: &[u8; 11]) -> bool {
    for i in 0..11 {
        if pat[i] != b'?' && pat[i] != name[i] {
            return false;
        }
    }
    true
}

/// The kernel's attribute test for a search (dos_findnext in
/// kernel/fatdir.c), mirrored so a redirected drive answers exactly
/// like a local one: read-only and archive bits never exclude an entry;
/// a search whose only special bit is the volume label matches labels
/// and nothing else.
pub fn attr_matches(attr_srch: u8, entry_attr: u8) -> bool {
    let srch = attr_srch & !(ATTR_READONLY | ATTR_ARCHIVE | ATTR_DEVICE);
    if srch == ATTR_VOLID {
        return entry_attr & ATTR_VOLID != 0;
    }
    (srch ^ 0xFF) & (ATTR_DIR | ATTR_SYSTEM | ATTR_HIDDEN | ATTR_VOLID) & entry_attr == 0
}

/// Split a fully qualified DOS path ("E:\GAMES\DOOM.TXT") into the
/// directory part including the drive ("E:\GAMES") and the final
/// component ("DOOM.TXT"). The directory of a root-level name is "E:\".
pub fn split_path(path: &[u8]) -> (&[u8], &[u8]) {
    let mut last = 0usize;
    for (i, &c) in path.iter().enumerate() {
        if c == b'\\' || c == b'/' {
            last = i;
        }
    }
    if last == 0 {
        (path, &path[path.len()..])
    } else if last == 2 && path.len() >= 2 && path[1] == b':' {
        (&path[..3], &path[3..])
    } else {
        (&path[..last], &path[last + 1..])
    }
}

/// Pack an exFAT timestamp into the DOS time/date words the kernel
/// stores in SFTs and directory entries.
pub fn dos_time_date(modified: u32) -> (u16, u16) {
    let (year, month, day, hour, min, sec) = exfat_ro::decode_timestamp(modified);
    let time = ((hour as u16) << 11) | ((min as u16) << 5) | ((sec as u16) / 2);
    let date = (((year.saturating_sub(1980)) as u16) << 9) | ((month as u16) << 5) | day as u16;
    (time, date)
}

// ---------------------------------------------------------------------------
// Cursor-based reading. Parameterised over the sector reader so host
// tests can drive it from a synthetic image; on target the reader is
// the USB mass-storage sector read.
// ---------------------------------------------------------------------------

/// Read `out` bytes of `ent` starting at file offset `pos`, advancing
/// the chain cursor (cluster, chain index, offset of that cluster's
/// first byte) as it goes. Returns the number of bytes read, short at
/// end of file. A cursor that sits past `pos` (a backward seek) is
/// reset to the chain head and walked forward again — seeks backward
/// are rare and reads are the hot path, so the cursor only ever moves
/// forward cheaply.
pub fn read_with_cursor<F: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    ent: &ExfatEntry,
    cursor: &mut (u32, u32, u64),
    pos: u64,
    out: &mut [u8],
    mut read_sec: F,
) -> usize {
    if pos >= ent.data_len || ent.first_cluster < 2 {
        return 0;
    }
    let want = core::cmp::min(out.len() as u64, ent.data_len - pos) as usize;
    let clus_bytes = (vol.sec_per_clus as u64) * 512;

    // Re-establish the cursor invariant: cursor.2 <= pos, and the
    // cursor cluster is the one containing cursor.2.
    if cursor.2 > pos || cursor.0 < 2 {
        *cursor = (ent.first_cluster, 0, 0);
    }
    while cursor.2 + clus_bytes <= pos {
        let next = if ent.no_fat_chain {
            cursor.0 + 1
        } else {
            match exfat_ro::next_cluster(vol, cursor.0, &mut read_sec) {
                Some(n) => n,
                None => return 0,
            }
        };
        cursor.0 = next;
        cursor.1 += 1;
        cursor.2 += clus_bytes;
    }

    let mut done = 0usize;
    let mut sec = [0u8; 512];
    while done < want {
        let cur = pos + done as u64;
        let in_clus = (cur - cursor.2) as usize;
        let sec_in_clus = in_clus / 512;
        let sec_off = in_clus % 512;
        let Some(base_lba) = exfat_ro::cluster_to_lba(vol, cursor.0) else {
            break;
        };
        if !read_sec(base_lba + sec_in_clus as u32, &mut sec) {
            break;
        }
        let n = core::cmp::min(512 - sec_off, want - done);
        out[done..done + n].copy_from_slice(&sec[sec_off..sec_off + n]);
        done += n;
        // Stepping into the next cluster moves the cursor with us, so
        // the next read (or the next sector of this one) starts warm.
        if in_clus + n >= clus_bytes as usize && done < want {
            let next = if ent.no_fat_chain {
                cursor.0 + 1
            } else {
                match exfat_ro::next_cluster(vol, cursor.0, &mut read_sec) {
                    Some(n) => n,
                    None => break,
                }
            };
            cursor.0 = next;
            cursor.1 += 1;
            cursor.2 += clus_bytes;
        }
    }
    done
}

// ---------------------------------------------------------------------------
// Request handling. One entry point serves every subfunction; the
// per-subfunction helpers below each return with the request block's
// output fields set (or with the consumed flag clear, to chain).
// ---------------------------------------------------------------------------

struct Req {
    lin: u32,
}

impl Req {
    fn get16(&self, off: usize) -> u16 {
        unsafe { rd16(self.lin + off as u32) }
    }
    fn set16(&self, off: usize, v: u16) {
        unsafe { wr16(self.lin + off as u32, v) }
    }
    fn get8(&self, off: usize) -> u8 {
        unsafe { rd8(self.lin + off as u32) }
    }
    fn fn_code(&self) -> u16 {
        self.get16(REQ_FN)
    }
    fn sft_lin(&self) -> u32 {
        far_lin(self.get16(REQ_ES), self.get16(REQ_DI))
    }
    fn sda_lin(&self) -> u32 {
        far_lin(self.get16(REQ_SDA_SEG), self.get16(REQ_SDA_OFF))
    }
    fn drive(&self) -> u8 {
        self.get8(REQ_DRIVE)
    }

    /// Mark the call answered, with carry set and AX = a DOS error code.
    fn fail(&self, err: u16) {
        self.set16(REQ_FN, err);
        self.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED | OUT_FLAG_CARRY);
    }
    /// Mark the call answered successfully; CX carries the result for
    /// the subfunctions whose kernel path reads it (the generic INT 2Fh
    /// path returns CX as the call's status, the read path as the byte
    /// count), so every success sets it explicitly.
    fn succeed(&self, cx: u16) {
        self.set16(REQ_FN, 0);
        self.set16(REQ_CX, cx);
        self.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
    }
    /// Leave the call unanswered: XREDIR restores the entry registers
    /// and chains to the previous INT 2Fh handler.
    fn chain(&self) {
        self.set16(REQ_OUT_FLAGS, 0);
    }
}

/// The mounted exFAT volume, if the USB medium currently holds one.
/// Also the media-change guard: a serial change since the open table
/// was built invalidates every record in it.
fn current_volume() -> Option<ExfatVolume> {
    let vol = ohci::redir_xvol()?;
    let st = redir_state();
    if st.vol_serial != vol.serial {
        st.vol_serial = vol.serial;
        // The new medium's flag state is its own; this session has
        // not dirtied it whatever the previous medium's state was.
        st.vol_dirty = false;
        for rec in st.opens.iter_mut() {
            rec.used = false;
        }
    }
    Some(vol)
}

/// The sector callbacks the writer core is parameterised over, bound
/// to the USB mass-storage plumbing. Reads and writes ride the same
/// Gate 3 BOT path EXFAT.COM and the safe-eject verb already use;
/// there is deliberately no second write path.
fn rd_sec(lba: u32, out: &mut [u8; 512]) -> bool {
    unsafe { ohci::msc_read_sector(lba, out) }
}
fn wr_sec(lba: u32, data: &[u8; 512]) -> bool {
    unsafe { ohci::msc_write_sector(lba, data) }
}

/// Set the volume's dirty flag at the first write of a session, so a
/// crash or power cut leaves the medium honestly marked for the next
/// mounter. Best-effort: a failed flag write does not block the write
/// that asked for it — the flag is bookkeeping, not data.
fn mark_vol_dirty(vol: &ExfatVolume) {
    if redir_state().vol_dirty {
        return;
    }
    if exfat_rw::set_volume_dirty(vol, true, rd_sec, wr_sec) {
        redir_state().vol_dirty = true;
    }
}

/// Clear the volume dirty flag once no open record still owes the
/// medium a stream-entry patch, and flush the device cache so the
/// clear — and everything written before it — is durable. Called from
/// the close/flush paths after they persist.
fn maybe_clear_dirty(vol: &ExfatVolume) {
    let st = redir_state();
    if !st.vol_dirty {
        return;
    }
    if st.opens.iter().any(|r| r.used && r.dirty) {
        return;
    }
    if exfat_rw::set_volume_dirty(vol, false, rd_sec, wr_sec) {
        unsafe { ohci::msc_sync_cache() };
        redir_state().vol_dirty = false;
    }
}

/// Rewrite one open record's on-disk stream entry from its live
/// entry/alloc state. This is the "entry last" step of the writer's
/// crash ordering: it runs only after the data, bitmap, and FAT
/// writes it describes are already on the medium. A record whose set
/// location is unknown is never dirty (its opens are read-only), so
/// the None arm only clears the flag defensively instead of retrying
/// a patch that can never be addressed.
fn persist_record(vol: &ExfatVolume, idx: usize) {
    let (loc, stream) = {
        let rec = &redir_state().opens[idx];
        if !rec.used || !rec.dirty {
            return;
        }
        match rec.loc {
            Some(loc) => (
                loc,
                Some((
                    rec.alloc.first_cluster,
                    rec.entry.data_len,
                    rec.entry.data_len,
                    rec.alloc.no_fat_chain,
                )),
            ),
            None => {
                redir_state().opens[idx].dirty = false;
                return;
            }
        }
    };
    if exfat_rw::patch_stream(vol, &loc, stream, None, rd_sec, wr_sec) {
        redir_state().opens[idx].dirty = false;
    }
}

/// Read the NUL-terminated PriPathName out of the SDA. The buffer is
/// 128 bytes; the kernel guarantees a terminator inside it.
fn read_pri_path(sda: u32, buf: &mut [u8; 128]) -> usize {
    let mut n = 0usize;
    while n < 128 {
        let c = unsafe { rd8(sda + SDA_PRI_PATH as u32 + n as u32) };
        if c == 0 {
            break;
        }
        buf[n] = c;
        n += 1;
    }
    n
}

/// Convert an ASCII DOS path (or path part) to the UTF-16 form the
/// exFAT parser compares. DOS paths are ASCII by construction here —
/// the kernel's truename has already rejected anything else.
fn ascii_to_u16(src: &[u8], dst: &mut [u16; 128]) -> usize {
    let n = core::cmp::min(src.len(), 128);
    for i in 0..n {
        dst[i] = src[i] as u16;
    }
    n
}

/// Resolve a fully qualified DOS path to its exFAT entry. `path` keeps
/// its "E:" prefix; the parser wants the path from the root, so the
/// drive prefix is skipped here. A bare root ("E:\") resolves to None
/// — callers that accept the root handle it before calling.
fn resolve_path(vol: &ExfatVolume, path: &[u8]) -> Option<ExfatEntry> {
    let rest = if path.len() >= 2 && path[1] == b':' {
        &path[2..]
    } else {
        path
    };
    if rest.is_empty() || (rest.len() == 1 && (rest[0] == b'\\' || rest[0] == b'/')) {
        return None;
    }
    let mut u16path = [0u16; 128];
    let n = ascii_to_u16(rest, &mut u16path);
    exfat_ro::find_path(vol, &u16path[..n], |lba, out| unsafe {
        ohci::msc_read_sector(lba, out)
    })
}

/// True when the path names this drive. Everything the kernel routes
/// here should, but a path that does not is another redirector's
/// business, so the handlers chain instead of answering.
fn path_is_mine(req: &Req, path: &[u8]) -> bool {
    path.len() >= 2 && path[1] == b':' && (path[0] | 0x20) == (b'a' + req.drive())
}

/// Fill the SFT the kernel prepared for a successful open. The kernel
/// has already zeroed it and set the owner PSP and mode; everything
/// about the file itself is the redirector's to write (dosfns.c
/// DosOpenSft returns as soon as the redirector call succeeds).
fn fill_sft(req: &Req, ent: &ExfatEntry, access: u8) {
    let sft = req.sft_lin();
    let name83 = dos_name_83(&ent.name[..ent.name_len as usize])
        .unwrap_or([b' '; 11]);
    let (time, date) = dos_time_date(ent.modified);
    unsafe {
        wr8(sft + SFT_ATTRIB as u32, (ent.attr & 0xFF) as u8);
        // FSHARED is what makes the kernel route later reads and closes
        // for this SFT back here. FCLEAN starts set only on a read-mode
        // open; do_write clears it after the first write through a
        // writable handle, which is the transition the bit exists for.
        let clean = if access == 0 { SFT_FCLEAN } else { 0 };
        wr16(
            sft + SFT_FLAGS as u32,
            SFT_FSHARED | clean | req.drive() as u16,
        );
        wr16(sft + SFT_TIME as u32, time);
        wr16(sft + SFT_DATE as u32, date);
        wr32(sft + SFT_SIZE as u32, ent.data_len as u32);
        wr32(sft + SFT_POSIT as u32, 0);
        copy_to_v86(sft + SFT_NAME as u32, &name83);
    }
}

fn open_record(
    req: &Req,
    ent: &ExfatEntry,
    access: u8,
    loc: Option<SetLoc>,
    alloc: Alloc,
) -> bool {
    let st = redir_state();
    let sft = req.sft_lin();
    for rec in st.opens.iter_mut() {
        if rec.used && rec.sft_lin == sft {
            rec.entry = *ent;
            rec.cursor_cluster = ent.first_cluster;
            rec.cursor_index = 0;
            rec.cursor_pos = 0;
            rec.psp = unsafe { rd16(sft + SFT_PSP as u32) };
            rec.access = access;
            rec.dirty = false;
            rec.loc = loc;
            rec.alloc = alloc;
            return true;
        }
    }
    for rec in st.opens.iter_mut() {
        if !rec.used {
            rec.used = true;
            rec.sft_lin = sft;
            rec.entry = *ent;
            rec.cursor_cluster = ent.first_cluster;
            rec.cursor_index = 0;
            rec.cursor_pos = 0;
            rec.psp = unsafe { rd16(sft + SFT_PSP as u32) };
            rec.access = access;
            rec.dirty = false;
            rec.loc = loc;
            rec.alloc = alloc;
            return true;
        }
    }
    false
}

fn find_record(sft: u32) -> Option<usize> {
    let st = redir_state();
    for (i, rec) in st.opens.iter().enumerate() {
        if rec.used && rec.sft_lin == sft {
            return Some(i);
        }
    }
    None
}

/// Ownership test for SFT-addressed calls. Opens stamp the SFT flags
/// with FSHARED plus this drive's number, so an SFT is this module's
/// exactly when both are present — including after CLOSEALL has dropped
/// the record, when the kernel still routes the handle closes here.
/// Anything else belongs to another redirector and must chain.
fn sft_is_mine(req: &Req) -> bool {
    let flags = unsafe { rd16(req.sft_lin() + SFT_FLAGS as u32) };
    flags & SFT_FSHARED != 0 && (flags & 0x003F) as u8 == req.drive()
}

// --- Subfunction handlers -------------------------------------------------

/// Create a new empty file named by the final component of `path`
/// (a path on this drive that does not exist yet) and open it with
/// the given access and attributes. Shared by EXTOC's create action
/// and the legacy CREATE subfunction, which differ only in where the
/// access and attributes came from. The name must be a plain 8.3
/// name — the only names DOS itself can carry — and the parent
/// directory must already exist (this stage creates files, not
/// directories). Errors are DOS error codes for the caller to fail
/// with.
fn create_and_open(
    req: &Req,
    vol: &ExfatVolume,
    path: &[u8],
    access: u8,
    attr: u16,
) -> Result<(), u16> {
    let (dir_part, comp) = split_path(path);
    let mut comp16 = [0u16; 128];
    let cn = ascii_to_u16(comp, &mut comp16);
    let comp16 = &comp16[..cn];
    if dos_name_83(comp16).is_none() {
        return Err(ERR_FILE_NOT_FOUND);
    }
    let (dir_cluster, dir_self) = if dir_part.len() <= 3 {
        (vol.root_cluster, None)
    } else {
        let mut d16 = [0u16; 128];
        let dn = ascii_to_u16(&dir_part[2..], &mut d16);
        match exfat_rw::locate_dir(vol, &d16[..dn], rd_sec) {
            Some(pair) => pair,
            None => return Err(ERR_PATH_NOT_FOUND),
        }
    };
    mark_vol_dirty(vol);
    let loc = exfat_rw::create_entry(
        vol,
        dir_cluster,
        dir_self,
        comp16,
        attr,
        CREATE_TIMESTAMP,
        rd_sec,
        wr_sec,
    )
    .ok_or(ERR_ACCESS_DENIED)?;
    // The entry the SFT and the record describe is exactly what
    // create_entry wrote: an empty file with the requested attributes.
    let mut name = [0u16; 128];
    name[..cn].copy_from_slice(comp16);
    let ent = ExfatEntry {
        name,
        name_len: cn as u8,
        name_truncated: false,
        attr,
        is_dir: false,
        first_cluster: 0,
        data_len: 0,
        valid_len: 0,
        no_fat_chain: false,
        modified: CREATE_TIMESTAMP,
    };
    fill_sft(req, &ent, access);
    if !open_record(req, &ent, access, Some(loc), Alloc::EMPTY) {
        return Err(ERR_TOO_MANY_OPEN);
    }
    Ok(())
}

/// OPEN (1116h) and EXTOC (112Eh): one resolution path, two admission
/// policies. The writer's `locate` is the primary resolver because an
/// open that may write needs the entry set's location, not just the
/// entry. Names the writer misses but the parser finds (long names
/// DOS cannot round-trip) can still be opened — read-only, since
/// there is no addressable set to patch. EXTOC additionally decodes
/// the action code: fail-if-exists, open, replace (truncate in
/// place), and the create bit, which makes a missing file through
/// `create_and_open`.
fn do_open(req: &Req, ext: bool) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    let path = &path[..n];
    if !path_is_mine(req, path) {
        return req.chain();
    }
    let (access, create, exists_action, create_attr) = if ext {
        let mode = unsafe { rd16(sda + SDA_EXT_MODE as u32) };
        let action = unsafe { rd16(sda + SDA_EXT_ACTION as u32) };
        let attr = unsafe { rd16(sda + SDA_EXT_ATTRIB as u32) };
        (
            (mode & 0x0003) as u8,
            action & 0x0010 != 0,
            (action & 0x0003) as u8,
            ((attr & 0x27) as u8) | ATTR_ARCHIVE,
        )
    } else {
        let mode = unsafe { rd8(sda + SDA_OPEN_MODE as u32) };
        (mode & 0x03, false, 1u8, 0u8)
    };
    let mut u16path = [0u16; 128];
    let un = ascii_to_u16(&path[2..], &mut u16path);
    let u16path = &u16path[..un];

    if let Some(loc) = exfat_rw::locate(&vol, u16path, rd_sec) {
        let ent = loc.entry;
        if ent.is_dir {
            return req.fail(ERR_ACCESS_DENIED);
        }
        if ext && exists_action == 0 {
            // Extended open with exists-action "fail": the file is
            // here, so the caller's own terms say the open must not
            // happen.
            return req.fail(ERR_ACCESS_DENIED);
        }
        if access != 0 && ent.attr & (ATTR_READONLY as u16) != 0 {
            // A read-only file refuses a write-mode open, as on any
            // DOS medium.
            return req.fail(ERR_ACCESS_DENIED);
        }
        let mut ent = ent;
        let mut alloc = Alloc::from_entry(&vol, &ent, rd_sec);
        if exists_action == 2 {
            // Replace: truncate in place before the open is granted.
            // The writer zeroes the stream entry before freeing the
            // chain, so no instant leaves the entry describing
            // clusters that are about to be released.
            mark_vol_dirty(&vol);
            if !exfat_rw::truncate_file(&vol, &loc.loc, &alloc, rd_sec, wr_sec) {
                return req.fail(ERR_ACCESS_DENIED);
            }
            ent.first_cluster = 0;
            ent.data_len = 0;
            ent.valid_len = 0;
            ent.no_fat_chain = false;
            alloc = Alloc::EMPTY;
        }
        fill_sft(req, &ent, access);
        if !open_record(req, &ent, access, Some(loc.loc), alloc) {
            return req.fail(ERR_TOO_MANY_OPEN);
        }
        // For EXTOC the kernel's return value reports the action taken
        // in its high word: 1 = opened, 3 = replaced. Legacy OPEN
        // ignores the value beyond its sign, and 0 is plain success.
        return req.succeed(if ext {
            if exists_action == 2 { 3 } else { 1 }
        } else {
            0
        });
    }

    // The writer's scan missed. The parser may still know the name
    // (a long name with characters a DOS path cannot carry): such a
    // file can be opened read-only, but never written, replaced, or
    // failed-if-exists with any honesty — there is no set location
    // to patch or truncate through.
    if let Some(ent) = resolve_path(&vol, path) {
        if ent.is_dir {
            return req.fail(ERR_ACCESS_DENIED);
        }
        if access != 0 || (ext && exists_action != 1) {
            return req.fail(ERR_ACCESS_DENIED);
        }
        fill_sft(req, &ent, 0);
        if !open_record(req, &ent, 0, None, Alloc::from_entry(&vol, &ent, rd_sec)) {
            return req.fail(ERR_TOO_MANY_OPEN);
        }
        return req.succeed(if ext { 1 } else { 0 });
    }

    if !create {
        return req.fail(ERR_FILE_NOT_FOUND);
    }
    match create_and_open(req, &vol, path, access, create_attr as u16) {
        // EXTOC action-taken code 2 = the file was created.
        Ok(()) => req.succeed(2),
        Err(e) => req.fail(e),
    }
}

/// CREATE (1117h) and CREATE-WITHOUT-CDS (1118h): the legacy create —
/// open the named file, truncating it if it exists, or create it
/// empty if it does not. The kernel has already prepared the SFT:
/// for the legacy path DosOpenSft stores the requested attributes
/// (pre-ORed with ARCHIVE) in sft_attrib before the call, and the
/// SDA carries the open mode (INT 21h 3Ch supplies read/write). This
/// kernel never emits 1118h, but the two are specified identically,
/// so they share this handler.
fn do_create(req: &Req) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    let path = &path[..n];
    if !path_is_mine(req, path) {
        return req.chain();
    }
    let access = unsafe { rd8(sda + SDA_OPEN_MODE as u32) } & 0x03;
    let attr = (unsafe { rd8(req.sft_lin() + SFT_ATTRIB as u32) } & 0x27) as u16;
    let mut u16path = [0u16; 128];
    let un = ascii_to_u16(&path[2..], &mut u16path);
    let u16path = &u16path[..un];

    if let Some(loc) = exfat_rw::locate(&vol, u16path, rd_sec) {
        let ent = loc.entry;
        if ent.is_dir {
            return req.fail(ERR_ACCESS_DENIED);
        }
        if ent.attr & (ATTR_READONLY as u16) != 0 {
            return req.fail(ERR_ACCESS_DENIED);
        }
        mark_vol_dirty(&vol);
        let alloc = Alloc::from_entry(&vol, &ent, rd_sec);
        if !exfat_rw::truncate_file(&vol, &loc.loc, &alloc, rd_sec, wr_sec) {
            return req.fail(ERR_ACCESS_DENIED);
        }
        let mut ent = ent;
        ent.first_cluster = 0;
        ent.data_len = 0;
        ent.valid_len = 0;
        ent.no_fat_chain = false;
        fill_sft(req, &ent, access);
        if !open_record(req, &ent, access, Some(loc.loc), Alloc::EMPTY) {
            return req.fail(ERR_TOO_MANY_OPEN);
        }
        return req.succeed(0);
    }
    match create_and_open(req, &vol, path, access, attr) {
        Ok(()) => req.succeed(0),
        Err(e) => req.fail(e),
    }
}

/// READ (1108h): the kernel points the SDA's DTA at the caller's buffer
/// and the SFT carries the position; the redirector moves the data and
/// advances the SFT position itself, because on the remote path the
/// kernel takes both from what the redirector leaves behind.
fn do_read(req: &Req) {
    let sda = req.sda_lin();
    let sft = req.sft_lin();
    if !sft_is_mine(req) {
        return req.chain();
    }
    let Some(idx) = find_record(sft) else {
        return req.fail(ERR_INVALID_HANDLE);
    };
    let Some(vol) = current_volume() else {
        return req.fail(ERR_INVALID_HANDLE);
    };
    let count = req.get16(REQ_CX) as usize;
    let pos = unsafe { rd32(sft + SFT_POSIT as u32) } as u64;
    let dta = far_lin(
        unsafe { rd16(sda + SDA_DTA as u32 + 2) },
        unsafe { rd16(sda + SDA_DTA as u32) },
    );
    if dta == 0 {
        return req.fail(ERR_INVALID_HANDLE);
    }
    // Read through one sector of stack at a time: the JLM's private
    // stack is 8 KiB and DOS read counts reach 64 KiB, so the data
    // moves in sector-sized hops from parser to DTA.
    let mut remaining = count;
    let mut total: u16 = 0;
    let mut sec = [0u8; 512];
    while remaining > 0 {
        let hop = core::cmp::min(remaining, 512);
        let n = {
            let rec = &mut redir_state().opens[idx];
            let mut cursor = (rec.cursor_cluster, rec.cursor_index, rec.cursor_pos);
            let got = read_with_cursor(
                &vol,
                &rec.entry,
                &mut cursor,
                pos + total as u64,
                &mut sec[..hop],
                |lba, out| unsafe { ohci::msc_read_sector(lba, out) },
            );
            rec.cursor_cluster = cursor.0;
            rec.cursor_index = cursor.1;
            rec.cursor_pos = cursor.2;
            got
        };
        if n == 0 {
            break;
        }
        unsafe { copy_to_v86(dta + total as u32, &sec[..n]) };
        total += n as u16;
        remaining -= n;
        if n < hop {
            break; // short read: end of file
        }
    }
    unsafe { wr32(sft + SFT_POSIT as u32, (pos + total as u64) as u32) };
    req.succeed(total);
}

/// WRITE (1109h): the mirror of READ — the SDA's DTA points at the
/// caller's buffer, the SFT carries the position, and the count
/// actually written goes back in CX. A write at or past end of file
/// extends the file: capacity is allocated first (the allocator
/// zeroes fresh clusters, and the gap between the old end and a
/// write position past it is zeroed explicitly, so gap bytes read
/// back as zeros — never as stale medium bytes), the data lands
/// second, and the stream entry on disk is patched only at
/// close/flush, which is the writer's entry-last ordering. A disk
/// that fills mid-write yields a short count; one that cannot take
/// a single byte yields DOS error 29 (write fault).
fn do_write(req: &Req) {
    let sda = req.sda_lin();
    let sft = req.sft_lin();
    if !sft_is_mine(req) {
        return req.chain();
    }
    let Some(idx) = find_record(sft) else {
        return req.fail(ERR_INVALID_HANDLE);
    };
    let Some(vol) = current_volume() else {
        return req.fail(ERR_INVALID_HANDLE);
    };
    if redir_state().opens[idx].access == 0 {
        // The handle was opened read-only; DOS refuses the write
        // itself, not just the open that asked for more.
        return req.fail(ERR_ACCESS_DENIED);
    }
    let count = req.get16(REQ_CX) as usize;
    if count == 0 {
        // A zero-length write is a successful no-op here. (DOS also
        // gives it a truncate-to-position meaning; this stage leaves
        // that out — truncation is available through create/replace.)
        return req.succeed(0);
    }
    let pos = unsafe { rd32(sft + SFT_POSIT as u32) } as u64;
    let dta = far_lin(
        unsafe { rd16(sda + SDA_DTA as u32 + 2) },
        unsafe { rd16(sda + SDA_DTA as u32) },
    );
    if dta == 0 {
        return req.fail(ERR_INVALID_HANDLE);
    }
    mark_vol_dirty(&vol);
    let clus_bytes = (vol.sec_per_clus as u64) * 512;
    let old_len = redir_state().opens[idx].entry.data_len;

    // Capacity first. ensure_capacity commits each growth round as
    // it goes, so even a false return leaves `alloc` honestly grown
    // part-way; the writable span is computed from what is actually
    // allocated, not from what was asked for.
    {
        let rec = &mut redir_state().opens[idx];
        let mut alloc = rec.alloc;
        exfat_rw::ensure_capacity(&vol, &mut alloc, pos + count as u64, rd_sec, wr_sec);
        rec.alloc = alloc;
    }
    let cap = redir_state().opens[idx].alloc.clusters as u64 * clus_bytes;
    if pos >= cap {
        return req.fail(ERR_WRITE_FAULT);
    }
    let writable = core::cmp::min(count as u64, cap - pos);

    // Zero the seek gap inside the (now sufficient) allocation.
    if pos > old_len {
        let alloc = redir_state().opens[idx].alloc;
        if !exfat_rw::zero_range(&vol, &alloc, old_len, pos, rd_sec, wr_sec) {
            return req.fail(ERR_WRITE_FAULT);
        }
    }

    // Move the data in sector hops from the DTA: the JLM stack is
    // 8 KiB and DOS write counts reach 64 KiB, exactly as for reads.
    let mut total: u64 = 0;
    let mut sec = [0u8; 512];
    while total < writable {
        let hop = core::cmp::min(512, (writable - total) as usize);
        unsafe { copy_from_v86(&mut sec[..hop], dta + total as u32) };
        let n = {
            let rec = &mut redir_state().opens[idx];
            let mut cursor = (rec.cursor_cluster, rec.cursor_index, rec.cursor_pos);
            let got = exfat_rw::write_with_cursor(
                &vol,
                &rec.alloc,
                &mut cursor,
                pos + total,
                &sec[..hop],
                rd_sec,
                wr_sec,
            );
            rec.cursor_cluster = cursor.0;
            rec.cursor_index = cursor.1;
            rec.cursor_pos = cursor.2;
            got
        };
        if n == 0 {
            break;
        }
        total += n as u64;
        if n < hop {
            break; // the chain ended early: a short write
        }
    }
    if total == 0 {
        return req.fail(ERR_WRITE_FAULT);
    }

    // The live record and the SFT move now; the on-disk stream entry
    // catches up at persist time (close/flush), per the ordering.
    let new_len = {
        let rec = &mut redir_state().opens[idx];
        let new_len = core::cmp::max(old_len, pos + total);
        rec.entry.data_len = new_len;
        rec.entry.valid_len = new_len;
        rec.entry.first_cluster = rec.alloc.first_cluster;
        rec.entry.no_fat_chain = rec.alloc.no_fat_chain;
        rec.dirty = true;
        new_len
    };
    unsafe {
        wr32(sft + SFT_POSIT as u32, (pos + total) as u32);
        wr32(sft + SFT_SIZE as u32, new_len as u32);
        let flags = rd16(sft + SFT_FLAGS as u32);
        wr16(sft + SFT_FLAGS as u32, flags & !SFT_FCLEAN);
    }
    req.succeed(total as u16);
}

/// DELETE (1113h): path-based, like GETATTRZ — the kernel passes no
/// handle for it. The writer invalidates the entry set first and
/// frees the clusters second, so a crash between the two leaks
/// clusters rather than dangling a live entry. Open records for the
/// deleted file are dropped with it: a later operation on the stale
/// handle must get invalid-handle, never write into freed clusters.
/// Directories and read-only files refuse with access denied, as on
/// a local DOS drive.
fn do_delete(req: &Req) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    let path = &path[..n];
    if !path_is_mine(req, path) {
        return req.chain();
    }
    let mut u16path = [0u16; 128];
    let un = ascii_to_u16(&path[2..], &mut u16path);
    let Some(loc) = exfat_rw::locate(&vol, &u16path[..un], rd_sec) else {
        return req.fail(ERR_FILE_NOT_FOUND);
    };
    let ent = loc.entry;
    if ent.is_dir || ent.attr & (ATTR_READONLY as u16) != 0 {
        return req.fail(ERR_ACCESS_DENIED);
    }
    mark_vol_dirty(&vol);
    let alloc = Alloc::from_entry(&vol, &ent, rd_sec);
    if !exfat_rw::delete_entry(&vol, &loc.loc, rd_sec, wr_sec) {
        return req.fail(ERR_ACCESS_DENIED);
    }
    let st = redir_state();
    for rec in st.opens.iter_mut() {
        if rec.used
            && rec.loc.is_some_and(|l| {
                l.dir_cluster == loc.loc.dir_cluster && l.set_off == loc.loc.set_off
            })
        {
            rec.used = false;
        }
    }
    exfat_rw::free_chain(&vol, &alloc, rd_sec, wr_sec);
    // The delete is complete and self-contained (no record carries
    // it as pending state), so make it durable now rather than
    // letting it ride on some later file's flush.
    unsafe { ohci::msc_sync_cache() };
    maybe_clear_dirty(&vol);
    req.succeed(0);
}

/// CLOSE (1106h): the kernel calls this once per handle close and
/// leaves the SFT reference count to the redirector ("remote sub
/// sft_count" in dosfns.c), so the count drops here and the record is
/// released only when the last handle goes. The last close persists
/// the record's stream entry (the entry-last step for everything the
/// handle wrote) before the record is released. A close for an SFT
/// with no record still succeeds: CLOSEALL and process-end sweeps
/// drop records first, and the kernel closes the handles afterwards
/// regardless.
fn do_close(req: &Req) {
    if !sft_is_mine(req) {
        return req.chain();
    }
    let sft = req.sft_lin();
    let count = unsafe { rd16(sft + SFT_COUNT as u32) };
    let new_count = count.saturating_sub(1);
    unsafe { wr16(sft + SFT_COUNT as u32, new_count) };
    if new_count == 0 {
        if let Some(idx) = find_record(sft) {
            if let Some(vol) = current_volume() {
                persist_record(&vol, idx);
                maybe_clear_dirty(&vol);
            }
            redir_state().opens[idx].used = false;
        }
    }
    req.succeed(0);
}

/// FLUSH (1107h): the kernel also routes its commit call here. A
/// flush of one of this module's SFTs persists the record's stream
/// entry and flushes the device cache, so a program that commits its
/// file gets the on-disk state it asked for; anyone else's chains.
fn do_flush(req: &Req) {
    if !sft_is_mine(req) {
        return req.chain();
    }
    if let Some(idx) = find_record(req.sft_lin()) {
        if let Some(vol) = current_volume() {
            persist_record(&vol, idx);
            unsafe { ohci::msc_sync_cache() };
            maybe_clear_dirty(&vol);
        }
    }
    req.succeed(0);
}

/// FLUSHALL (1120h): persist every dirty record and flush the
/// device, then chain — like CLOSEALL, this call belongs to every
/// redirector in the chain, so handling ours does not consume it.
fn do_flushall(req: &Req) {
    if let Some(vol) = current_volume() {
        for idx in 0..MAX_OPEN {
            persist_record(&vol, idx);
        }
        unsafe { ohci::msc_sync_cache() };
        maybe_clear_dirty(&vol);
    }
    req.chain();
}

/// LSEEK (1121h): this kernel calls it only for a seek-from-end on a
/// shared file, with the requested absolute position already computed
/// in CX:DX; the answer is the new absolute position in DX:AX. The
/// kernel stores it into the SFT itself.
fn do_lseek(req: &Req) {
    if !sft_is_mine(req) {
        return req.chain();
    }
    let sft = req.sft_lin();
    if find_record(sft).is_none() {
        return req.fail(ERR_INVALID_HANDLE);
    }
    let new_pos = ((req.get16(REQ_CX) as u32) << 16) | req.get16(REQ_DX) as u32;
    req.set16(REQ_FN, (new_pos & 0xFFFF) as u16);
    req.set16(REQ_DX, (new_pos >> 16) as u16);
    req.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
}

/// GETSPACE (110Ch): ES:DI is this drive's CDS. Space comes from the
/// volume geometry plus a walk of the allocation bitmap — the only
/// numbers DOS can act on, in the register order int2f.asm's
/// remote_getfree stores them (AX sectors/cluster, BX total clusters,
/// CX bytes/sector, DX free clusters).
fn do_getspace(req: &Req) {
    // ES:DI is the CDS the kernel is asking about; its current path
    // starts with the drive letter, which is the ownership test that
    // does not depend on how either side normalised the far pointer.
    if unsafe { rd8(req.sft_lin()) } != b'A' + req.drive() {
        return req.chain();
    }
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let free = exfat_ro::free_clusters(&vol, |lba, out| unsafe {
        ohci::msc_read_sector(lba, out)
    })
    .unwrap_or(0);
    req.set16(REQ_FN, vol.sec_per_clus as u16);
    req.set16(REQ_BX, vol.cluster_count as u16);
    req.set16(REQ_CX, 512);
    req.set16(REQ_DX, free as u16);
    req.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
}

/// GETATTRZ (110Fh): attributes for a path. This kernel reads only AX
/// back; the RBIL-documented size/time/date registers are filled
/// anyway, so the answer stays correct for any other consumer of the
/// multiplex chain.
fn do_getattr(req: &Req) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    let path = &path[..n];
    if !path_is_mine(req, path) {
        return req.chain();
    }
    let ent = resolve_path(&vol, path);
    let Some(ent) = ent else {
        // A bare root never reaches here (the kernel short-circuits
        // it), but answer it correctly if one ever does.
        if path.len() <= 3 {
            req.set16(REQ_FN, ATTR_DIR as u16);
            req.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
            return;
        }
        return req.fail(ERR_FILE_NOT_FOUND);
    };
    let (time, date) = dos_time_date(ent.modified);
    req.set16(REQ_FN, (ent.attr & 0xFF) as u16);
    req.set16(REQ_BX, (ent.data_len >> 16) as u16);
    req.set16(REQ_DI, (ent.data_len & 0xFFFF) as u16);
    req.set16(REQ_CX, time);
    req.set16(REQ_DX, date);
    req.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
}

/// CHDIR (1105h): the kernel copies the qualified path into the CDS
/// itself once the redirector agrees the directory exists, so this is
/// a pure validation — there is no per-drive current-directory state
/// on this side to keep in sync.
fn do_chdir(req: &Req) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    let path = &path[..n];
    if !path_is_mine(req, path) {
        return req.chain();
    }
    if path.len() <= 3 {
        return req.succeed(0); // the root always exists
    }
    match resolve_path(&vol, path) {
        Some(ent) if ent.is_dir => req.succeed(0),
        _ => req.fail(ERR_PATH_NOT_FOUND),
    }
}

/// Write the 32-byte SearchDir entry the kernel copies to the caller's
/// DTA (pop_dmp in dosfns.c reads name, attribute, time, date, size).
fn fill_search_dir(sda: u32, name83: &[u8; 11], attr: u8, modified: u32, size: u64) {
    let (time, date) = dos_time_date(modified);
    let lin = sda + SDA_SEARCH_DIR as u32;
    unsafe {
        copy_to_v86(lin, name83);
        wr8(lin + 0x0B, attr);
        wr16(lin + 0x16, time);
        wr16(lin + 0x18, date);
        wr32(lin + 0x1C, size as u32);
    }
}

/// FINDFIRST (111Bh) / FINDNEXT (111Ch): one scan routine. The state
/// lives in the kernel's 21-byte dmatch record — it is the only storage
/// that round-trips through the caller's DTA between the two calls —
/// holding the FCB pattern the kernel itself prepared in DirEntBuffer,
/// the directory's first cluster, and how many raw entries the scan
/// has already examined. FINDNEXT re-walks the directory and skips
/// that many entries; directories on a removable stick are small, and
/// the record format leaves no room for a chain cursor.
fn do_find(req: &Req, first: bool) {
    let sda = req.sda_lin();
    let Some(vol) = current_volume() else {
        return req.fail(ERR_PATH_NOT_FOUND);
    };
    // (state_lin, pattern, attr_srch, dir_cluster, examined)
    let (state_lin, mut pat, attr_srch, dir_cluster, mut examined);
    if first {
        state_lin = sda + SDA_TMP_DM as u32;
        let mut path = [0u8; 128];
        let n = read_pri_path(sda, &mut path);
        let path = &path[..n];
        if !path_is_mine(req, path) {
            return req.chain();
        }
        // The kernel prepared the FCB form of the final component in
        // DirEntBuffer (set_fcbname before the redirector call); take
        // it verbatim rather than re-deriving DOS's conversion rules.
        pat = [0u8; 11];
        unsafe { copy_from_v86(&mut pat, sda + SDA_DIR_ENT_BUF as u32) };
        attr_srch = unsafe { rd8(sda + SDA_SATTR as u32) };
        let (dir_part, _comp) = split_path(path);
        dir_cluster = if dir_part.len() <= 3 {
            vol.root_cluster
        } else {
            match resolve_path(&vol, dir_part) {
                Some(ent) if ent.is_dir => ent.first_cluster,
                _ => return req.fail(ERR_PATH_NOT_FOUND),
            }
        };
        examined = 0;
        // Publish the state before answering: the kernel copies the
        // whole record to the caller's DTA on success and on failure
        // alike, and a later FINDNEXT trusts it completely.
        unsafe {
            wr8(state_lin + DM_DRIVE as u32, 0xC0 | req.drive());
            copy_to_v86(state_lin + DM_NAME_PAT as u32, &pat);
            wr8(state_lin + DM_ATTR_SRCH as u32, attr_srch);
            wr16(state_lin + DM_ENTRY as u32, 0);
            wr32(state_lin + DM_DIRCLUSTER as u32, dir_cluster);
            wr8(state_lin + DM_STAGE as u32, 0);
        }
        // A label-only search is answered by the volume label itself,
        // never by scanning: exFAT keeps no label entry in directories
        // for the scan to find (the label lives in the root's system
        // entries, which the parser exposes as volume metadata).
        if attr_matches_label_only(attr_srch) {
            if vol.label_len == 0 {
                return req.fail(ERR_NO_MORE_FILES);
            }
            let mut name = [b' '; 11];
            let mut ok = true;
            for i in 0..core::cmp::min(vol.label_len as usize, 11) {
                let ch = vol.label[i];
                if !(0x20..=0x7E).contains(&ch) {
                    ok = false;
                    break;
                }
                name[i] = ch as u8;
            }
            if !ok {
                return req.fail(ERR_NO_MORE_FILES);
            }
            fill_search_dir(sda, &name, ATTR_VOLID, 0, 0);
            unsafe { wr8(state_lin + DM_STAGE as u32, 1) };
            return req.succeed(0);
        }
    } else {
        state_lin = req.sft_lin(); // ES:DI = &sda_tmp_dm on FINDNEXT
        if unsafe { rd8(state_lin + DM_DRIVE as u32) } != 0xC0 | req.drive() {
            return req.chain();
        }
        pat = [0u8; 11];
        unsafe { copy_from_v86(&mut pat, state_lin + DM_NAME_PAT as u32) };
        attr_srch = unsafe { rd8(state_lin + DM_ATTR_SRCH as u32) };
        dir_cluster = unsafe { rd32(state_lin + DM_DIRCLUSTER as u32) };
        examined = unsafe { rd16(state_lin + DM_ENTRY as u32) };
        let stage = unsafe { rd8(state_lin + DM_STAGE as u32) };
        if stage != 0 || attr_matches_label_only(attr_srch) {
            return req.fail(ERR_NO_MORE_FILES);
        }
    }

    let mut seen: u16 = 0;
    let mut hit: Option<ExfatEntry> = None;
    exfat_ro::for_each_dir_entry(
        &vol,
        dir_cluster,
        |lba, out| unsafe { ohci::msc_read_sector(lba, out) },
        |e| {
            if seen < examined {
                seen += 1;
                return true;
            }
            seen += 1;
            let Some(name83) = dos_name_83(&e.name[..e.name_len as usize]) else {
                return true; // unrepresentable in 8.3: skipped by design
            };
            if !fcb_match(&pat, &name83) {
                return true;
            }
            if !attr_matches(attr_srch, (e.attr & 0xFF) as u8) {
                return true;
            }
            hit = Some(*e);
            false
        },
    );
    examined = seen;
    unsafe { wr16(state_lin + DM_ENTRY as u32, examined) };
    let Some(ent) = hit else {
        return req.fail(ERR_NO_MORE_FILES);
    };
    let name83 = dos_name_83(&ent.name[..ent.name_len as usize]).unwrap_or([b' '; 11]);
    fill_search_dir(
        sda,
        &name83,
        (ent.attr & 0xFF) as u8,
        ent.modified,
        ent.data_len,
    );
    req.succeed(0)
}

/// The kernel's label-search test: with read-only, archive and device
/// bits ignored, a search attribute of exactly ATTR_VOLID asks for the
/// volume label and nothing else.
fn attr_matches_label_only(attr_srch: u8) -> bool {
    attr_srch & !(ATTR_READONLY | ATTR_ARCHIVE | ATTR_DEVICE) == ATTR_VOLID
}

/// CLOSEALL (111Dh): persist every dirty record, drop every open
/// record, then chain — every redirector in the chain owns files of
/// its own and must hear this call, so handling ours does not consume
/// it. The kernel still closes each handle afterwards; those closes
/// find no record but a stamped SFT, and succeed quietly (see
/// do_close), which is what makes the pair safe in any order.
fn do_closeall(req: &Req) {
    if let Some(vol) = current_volume() {
        for idx in 0..MAX_OPEN {
            persist_record(&vol, idx);
        }
        maybe_clear_dirty(&vol);
    }
    let st = redir_state();
    for rec in st.opens.iter_mut() {
        rec.used = false;
    }
    req.chain();
}

/// PROCESS_END (1122h): the kernel sets DS to the exiting process's
/// PSP before the call (remote_process_end in int2f.asm). Records
/// record their owner PSP at open, so the sweep is exact. Like
/// CLOSEALL this call belongs to every redirector, so it chains after
/// the sweep.
fn do_process_end(req: &Req) {
    let psp = req.get16(REQ_DS);
    // A process that exits with files open never closes them itself;
    // persist its dirty records before the sweep drops them, or
    // everything it wrote past the last flush would describe clusters
    // the on-disk entry does not know about.
    let vol = current_volume();
    if let Some(vol) = &vol {
        for idx in 0..MAX_OPEN {
            let mine = redir_state().opens[idx].used && redir_state().opens[idx].psp == psp;
            if mine {
                persist_record(vol, idx);
            }
        }
    }
    let st = redir_state();
    for rec in st.opens.iter_mut() {
        if rec.used && rec.psp == psp {
            rec.used = false;
        }
    }
    if let Some(vol) = &vol {
        maybe_clear_dirty(vol);
    }
    req.chain();
}

// --- FFI entries ------------------------------------------------------------

/// JLM V86 API verb 0x0030: serve one redirector request block.
/// ES:DI at the API call points at the block XREDIR.COM marshalled;
/// the stub converts it to the linear address passed here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_redir_call(req_lin: u32) -> u32 {
    if req_lin == 0 {
        return 0;
    }
    let req = Req { lin: req_lin };
    match req.fn_code() {
        0x1100 => {
            // Install check: a redirector answers AL = 0xFF.
            req.set16(REQ_FN, 0x11FF);
            req.set16(REQ_OUT_FLAGS, OUT_FLAG_CONSUMED);
        }
        0x1105 => do_chdir(&req),
        0x1106 => do_close(&req),
        0x1107 => do_flush(&req),
        0x1108 => do_read(&req),
        0x1109 => do_write(&req),
        // LOCK/UNLOCK: no lock state is kept. Refuse ours explicitly —
        // leaving them unclaimed would hand the kernel an un-negated
        // function number as the "error".
        0x110A | 0x110B => {
            if sft_is_mine(&req) {
                req.fail(ERR_INVALID_FUNCTION);
            } else {
                req.chain();
            }
        }
        0x110C => do_getspace(&req),
        0x110F => do_getattr(&req),
        0x1113 => do_delete(&req),
        0x1116 => do_open(&req, false),
        0x1117 | 0x1118 => do_create(&req),
        0x111B => do_find(&req, true),
        0x111C => do_find(&req, false),
        0x111D => do_closeall(&req),
        0x1120 => do_flushall(&req),
        0x1121 => do_lseek(&req),
        0x1122 => do_process_end(&req),
        // The directory-mutation family stays refused in this stage:
        // MKDIR, RMDIR, RENAME, and SETATTR are access-denied error
        // returns, never writes. Files are created and deleted;
        // directories are not, yet.
        0x1101 | 0x1103 | 0x110E | 0x1111 => {
            // Only refuse what is actually ours: a call naming
            // another redirector's drive belongs to its owner.
            if refused_call_is_mine(&req) {
                req.fail(ERR_ACCESS_DENIED);
            } else {
                req.chain();
            }
        }
        // FILENAME (1123h) is deliberately not answered: truename calls
        // it for every path on every drive before resolving anything,
        // and a success claim here would have the kernel trust this
        // module's canonicalisation for foreign drives too. Chaining
        // (and the error the chain ends in) makes the kernel fall back
        // to its own canonicalisation, which handles CDS drives
        // correctly on its own.
        _ => req.chain(),
    }
    0
}

/// Ownership test for the refused family: all four are path-based
/// calls, so the drive named in PriPathName decides.
fn refused_call_is_mine(req: &Req) -> bool {
    let sda = req.sda_lin();
    let mut path = [0u8; 128];
    let n = read_pri_path(sda, &mut path);
    path_is_mine(req, &path[..n])
}

/// JLM V86 API verb 0x0031: install-time volume query for XREDIR.COM.
/// Fills the 32-byte info block at `info_lin` (present flag, serial,
/// ASCII label) and returns 1 when an exFAT volume is mounted, else 0.
/// XREDIR refuses to install on a 0: a drive letter for a volume that
/// is not there would only fail every call made against it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_redir_query(info_lin: u32) -> u32 {
    if info_lin == 0 {
        return 0;
    }
    let Some(vol) = current_volume() else {
        unsafe { wr16(info_lin, 0) };
        return 0;
    };
    unsafe {
        wr16(info_lin, 1);
        wr32(info_lin + 2, vol.serial);
        wr8(info_lin + 6, vol.label_len);
        for i in 0..11usize {
            let ch = if i < vol.label_len as usize {
                vol.label[i]
            } else {
                0
            };
            let b = if (0x20..=0x7E).contains(&ch) {
                ch as u8
            } else if ch == 0 {
                0
            } else {
                b'?'
            };
            wr8(info_lin + 7 + i as u32, b);
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16name(s: &str) -> Vec<u16> {
        s.chars().map(|c| c as u16).collect()
    }

    #[test]
    fn dos_name_83_converts_plain_names() {
        assert_eq!(
            dos_name_83(&u16name("HELLO.TXT")),
            Some(*b"HELLO   TXT")
        );
        assert_eq!(dos_name_83(&u16name("GAMES")), Some(*b"GAMES      "));
        assert_eq!(dos_name_83(&u16name("A.B")), Some(*b"A       B  "));
        assert_eq!(
            dos_name_83(&u16name("ABCDEFGH.IJK")),
            Some(*b"ABCDEFGHIJK")
        );
    }

    #[test]
    fn dos_name_83_upcases_lowercase() {
        // DOS names are uppercase; exFAT preserves case. The 8.3 form
        // of a lowercase name is its uppercase folding.
        assert_eq!(
            dos_name_83(&u16name("doom.txt")),
            Some(*b"DOOM    TXT")
        );
        assert_eq!(
            dos_name_83(&u16name("Hello.Txt")),
            Some(*b"HELLO   TXT")
        );
    }

    #[test]
    fn dos_name_83_rejects_unrepresentable_names() {
        // Too long in either field.
        assert_eq!(dos_name_83(&u16name("LongFileNameTest.txt")), None);
        assert_eq!(dos_name_83(&u16name("ABCDEFGHI.TXT")), None);
        assert_eq!(dos_name_83(&u16name("FILE.TEXT")), None);
        // A second dot has no 8.3 meaning.
        assert_eq!(dos_name_83(&u16name("foo.bar.txt")), None);
        // Leading dot, space, and DOS-forbidden punctuation.
        assert_eq!(dos_name_83(&u16name(".HIDDEN")), None);
        assert_eq!(dos_name_83(&u16name("MY FILE.TXT")), None);
        assert_eq!(dos_name_83(&u16name("A+B.TXT")), None);
        assert_eq!(dos_name_83(&u16name("A[B].TXT")), None);
        // Non-ASCII cannot be a DOS name byte.
        assert_eq!(dos_name_83(&u16name("CAFÉ.TXT")), None);
        assert_eq!(dos_name_83(&u16name("")), None);
    }

    #[test]
    fn fcb_match_uses_kernel_wildcard_rules() {
        let hello = *b"HELLO   TXT";
        assert!(fcb_match(b"???????????", &hello));
        assert!(fcb_match(b"HELLO   TXT", &hello));
        assert!(fcb_match(b"H?LLO   T?T", &hello));
        assert!(!fcb_match(b"HELLO   BIN", &hello));
        assert!(!fcb_match(b"XELLO   TXT", &hello));
    }

    #[test]
    fn attr_matches_mirrors_kernel_search_rules() {
        // A plain search finds plain files (archive/readonly are free).
        assert!(attr_matches(0x00, ATTR_ARCHIVE));
        assert!(attr_matches(0x00, ATTR_READONLY | ATTR_ARCHIVE));
        // ...but not directories, hidden, system, or label entries.
        assert!(!attr_matches(0x00, ATTR_DIR));
        assert!(!attr_matches(0x00, ATTR_HIDDEN));
        assert!(!attr_matches(0x00, ATTR_VOLID));
        // Asking for the bits admits them.
        assert!(attr_matches(0x16, ATTR_DIR | ATTR_ARCHIVE));
        assert!(attr_matches(0x16, ATTR_HIDDEN | ATTR_SYSTEM));
        // A label-only search matches labels and nothing else.
        assert!(attr_matches(ATTR_VOLID, ATTR_VOLID));
        assert!(!attr_matches(ATTR_VOLID, ATTR_ARCHIVE));
        assert!(!attr_matches(ATTR_VOLID, ATTR_DIR));
        // A search with every bit set finds labels too.
        assert!(attr_matches(0x3F, ATTR_VOLID));
    }

    #[test]
    fn split_path_separates_directory_and_name() {
        assert_eq!(
            split_path(b"E:\\GAMES\\DOOM.TXT"),
            (&b"E:\\GAMES"[..], &b"DOOM.TXT"[..])
        );
        assert_eq!(
            split_path(b"E:\\HELLO.TXT"),
            (&b"E:\\"[..], &b"HELLO.TXT"[..])
        );
        assert_eq!(split_path(b"E:\\"), (&b"E:\\"[..], &b""[..]));
    }

    #[test]
    fn dos_time_date_packs_dos_words() {
        // 2026-11-24 13:45:30 in the exFAT timestamp packing.
        let modified = (15u32) | (45 << 5) | (13 << 11) | (24 << 16) | (11 << 21) | (46 << 25);
        let (time, date) = dos_time_date(modified);
        assert_eq!(time, (13 << 11) | (45 << 5) | 15);
        assert_eq!(date, (46 << 9) | (11 << 5) | 24);
    }

    // --- Cursor reads over a synthetic image ---------------------------
    //
    // 512-byte sectors and clusters; FAT at sector 24, heap at 28, the
    // same skeleton the exfat_ro tests use. One file scattered across
    // clusters 5 -> 8 -> 3 by its FAT chain, one contiguous file at
    // clusters 10 -> 11 -> 12.

    const FAT_LBA: u32 = 24;
    const HEAP_LBA: u32 = 28;

    fn coff(clus: u32) -> usize {
        (HEAP_LBA as usize + (clus as usize - 2)) * 512
    }

    fn test_volume() -> ExfatVolume {
        ExfatVolume {
            part_lba: 0,
            sec_per_clus: 1,
            fat_lba: FAT_LBA,
            fat_secs: 4,
            heap_lba: HEAP_LBA,
            cluster_count: 16,
            root_cluster: 4,
            serial: 0xCAFE,
            bitmap_cluster: 2,
            bitmap_len: 2,
            upcase_cluster: 3,
            upcase_len: 5836,
            upcase_checksum: 0,
            label: [0; 11],
            label_len: 0,
        }
    }

    fn test_image() -> Vec<u8> {
        let mut img = vec![0u8; (HEAP_LBA as usize + 16) * 512];
        let fat = FAT_LBA as usize * 512;
        let set_fat = |img: &mut [u8], clus: u32, next: u32| {
            let off = fat + clus as usize * 4;
            img[off..off + 4].copy_from_slice(&next.to_le_bytes());
        };
        set_fat(&mut img, 5, 8);
        set_fat(&mut img, 8, 3);
        set_fat(&mut img, 3, 0xFFFF_FFFF);
        // Scattered file payload: cluster 5 = 'A', 8 = 'B', 3 = 'C'.
        for i in 0..512 {
            img[coff(5) + i] = b'A';
            img[coff(8) + i] = b'B';
            img[coff(3) + i] = b'C';
        }
        // Contiguous file payload at 10, 11, 12: 'x', 'y', 'z'.
        for i in 0..512 {
            img[coff(10) + i] = b'x';
            img[coff(11) + i] = b'y';
            img[coff(12) + i] = b'z';
        }
        img
    }

    fn entry(first_cluster: u32, data_len: u64, no_fat_chain: bool) -> ExfatEntry {
        ExfatEntry {
            name: [0; 128],
            name_len: 0,
            name_truncated: false,
            attr: 0x20,
            is_dir: false,
            first_cluster,
            data_len,
            valid_len: data_len,
            no_fat_chain,
            modified: 0,
        }
    }

    fn reader<'a>(img: &'a [u8]) -> impl FnMut(u32, &mut [u8; 512]) -> bool + 'a {
        move |lba, out| {
            let off = lba as usize * 512;
            out.copy_from_slice(&img[off..off + 512]);
            true
        }
    }

    #[test]
    fn cursor_read_walks_scattered_chain_in_order() {
        let vol = test_volume();
        let img = test_image();
        let ent = entry(5, 1300, false);
        let mut cursor = (0, 0, 0);
        let mut buf = [0u8; 512];
        // Three sequential hops, as do_read issues them.
        let n1 = read_with_cursor(&vol, &ent, &mut cursor, 0, &mut buf, reader(&img));
        assert_eq!(n1, 512);
        assert!(buf.iter().all(|&b| b == b'A'));
        let n2 = read_with_cursor(&vol, &ent, &mut cursor, 512, &mut buf, reader(&img));
        assert_eq!(n2, 512);
        assert!(buf.iter().all(|&b| b == b'B'));
        let n3 = read_with_cursor(&vol, &ent, &mut cursor, 1024, &mut buf, reader(&img));
        assert_eq!(n3, 276); // short at EOF
        assert!(buf[..276].iter().all(|&b| b == b'C'));
        let n4 = read_with_cursor(&vol, &ent, &mut cursor, 1300, &mut buf, reader(&img));
        assert_eq!(n4, 0);
    }

    #[test]
    fn cursor_read_handles_mid_cluster_start_and_backward_seek() {
        let vol = test_volume();
        let img = test_image();
        let ent = entry(5, 1300, false);
        let mut cursor = (0, 0, 0);
        let mut buf = [0u8; 100];
        // Start 100 bytes into the second cluster (chain hop 8 = 'B').
        let n = read_with_cursor(&vol, &ent, &mut cursor, 612, &mut buf, reader(&img));
        assert_eq!(n, 100);
        assert!(buf.iter().all(|&b| b == b'B'));
        // A read spanning the cluster boundary mixes both payloads.
        let mut big = [0u8; 200];
        let n = read_with_cursor(&vol, &ent, &mut cursor, 950, &mut big, reader(&img));
        assert_eq!(n, 200);
        assert!(big[..74].iter().all(|&b| b == b'B'));
        assert!(big[74..].iter().all(|&b| b == b'C'));
        // Seeking back to the start resets the cursor to the head.
        let n = read_with_cursor(&vol, &ent, &mut cursor, 0, &mut buf, reader(&img));
        assert_eq!(n, 100);
        assert!(buf.iter().all(|&b| b == b'A'));
    }

    #[test]
    fn cursor_read_handles_contiguous_files() {
        let vol = test_volume();
        let img = test_image();
        let ent = entry(10, 1200, true);
        let mut cursor = (0, 0, 0);
        let mut buf = [0u8; 1200];
        let n = read_with_cursor(&vol, &ent, &mut cursor, 0, &mut buf, reader(&img));
        assert_eq!(n, 1200);
        assert!(buf[..512].iter().all(|&b| b == b'x'));
        assert!(buf[512..1024].iter().all(|&b| b == b'y'));
        assert!(buf[1024..].iter().all(|&b| b == b'z'));
    }
}
