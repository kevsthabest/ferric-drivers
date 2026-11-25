//! NTFS read-only parser — Stage E filesystem core (HANDOFF NTFS item,
//! owner-directed 2026-11-24: read-only first).
//!
//! Pure `#![no_std]`, zero-heap, sector-callback reader in the same shape
//! as `fat_ro.rs` / `exfat_ro.rs`. Contains ZERO write logic by
//! construction. Format knowledge comes from the ntfs-3g / linux-ntfs
//! reverse-engineering corpus — NTFS was never officially specified.
//!
//! Trust model: everything on the medium is untrusted input. Every MFT
//! record and every INDX block is update-sequence (fixup) validated
//! before a single field is read from it — a record whose sector tails
//! do not match its update sequence number is rejected, never trusted.
//! All offsets and lengths are checked before use.
//!
//! Record buffers are supplied by the caller (`rec_buf`, `indx_buf`)
//! and must be at least the volume's record size. NTFS records run to
//! 4 KiB, and the JLM serves these calls on an 8 KiB private stack —
//! the exFAT boot-checksum hang (Stage B) is what happens when a
//! multi-KiB buffer lands on that stack, so no record-sized array is
//! ever a local in this module. In the JLM the buffers live in the
//! UsbMscState statics; in host tests they are ordinary locals.
//!
//! v1 limits, stated plainly:
//! - 512-byte-sector volumes only (the sector callback this crate
//!   standardised on), MBR partitioning, record and index-block sizes
//!   up to 4 KiB.
//! - No $ATTRIBUTE_LIST following: a file fragmented across extension
//!   MFT records exposes only the attributes stored in its base record.
//! - Compressed and encrypted $DATA is refused, not decoded.
//! - The $MFT runlist must fit in 256 bytes (dozens of runs; a $MFT
//!   fragmented beyond that is not mountable by v1).
//! - Name case folding uses a built-in ASCII + Latin-1 subset rather
//!   than the volume's $UpCase record; DOS-era names are ASCII, which
//!   the subset covers exactly.
//! - Names longer than 128 UTF-16 code units are reported truncated.
//! - Directory enumeration reports the directory index as the
//!   filesystem maintains it. A file's index entries are removed by
//!   the filesystem when the file is deleted, so the index is the live
//!   view; `find_path` and the read path additionally verify the
//!   target record's in-use flag, so a stale index entry left behind
//!   by a crash cannot make a deleted file readable.

/// MFT record number of the root directory ("." — fixed by the format).
pub const ROOT_RECORD: u64 = 5;
/// MFT record number of $Volume (fixed by the format).
const VOLUME_RECORD: u64 = 3;
/// MFT record number of $Bitmap, the cluster allocation bitmap.
const BITMAP_RECORD: u64 = 6;

/// Largest MFT record / index block this parser will buffer (4 KiB).
pub const MAX_RECORD_SIZE: u32 = 4096;

/// $FILE_NAME namespaces (the byte at offset 0x41 of the value).
const NS_POSIX: u8 = 0;
const NS_WIN32: u8 = 1;
const NS_DOS: u8 = 2;
const NS_WIN32_DOS: u8 = 3;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct NtfsVolume {
    pub part_lba: u32,
    pub sec_per_clus: u32,
    pub total_sectors: u64,
    pub cluster_count: u64,
    pub mft_lcn: u64,
    pub mft_record_size: u32,
    pub index_record_size: u32,
    pub serial: u64,
    /// $MFT's own $DATA: real size and raw runlist bytes, captured at
    /// mount so any record can be located without re-reading record 0.
    pub mft_data_len: u64,
    pub mft_runlist: [u8; 256],
    pub mft_runlist_len: u16,
    /// Volume label from $Volume's $VOLUME_NAME (empty when absent).
    pub label: [u16; 32],
    pub label_len: u8,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct NtfsEntry {
    pub name: [u16; 128],
    pub name_len: u8,
    pub name_truncated: bool,
    /// MFT record number of the file.
    pub record: u64,
    pub is_dir: bool,
    /// Real size of the unnamed $DATA stream.
    pub data_len: u64,
    /// Raw FILETIME modification stamp from $FILE_NAME.
    pub modified: u64,
}

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn r32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn r64(b: &[u8], off: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..8 {
        v |= (b[off + i] as u64) << (8 * i);
    }
    v
}

/// The boot sector's two size bytes (clusters per MFT record at 0x40,
/// clusters per index block at 0x44) are signed: a positive value is a
/// count of clusters, a negative value means the size is
/// 2^(-value) bytes. Zero is invalid.
fn decode_size_byte(v: u8, cluster_bytes: u32) -> Option<u32> {
    let s = v as i8;
    if s > 0 {
        (s as u32).checked_mul(cluster_bytes)
    } else if s < 0 {
        1u32.checked_shl((-s) as u32)
    } else {
        None
    }
}

/// Parse an NTFS boot sector (the partition's first sector) into volume
/// geometry. The $MFT runlist and label are left empty; use
/// `mount_volume` for a fully populated volume.
pub fn parse_boot(sec: &[u8; 512], part_lba: u32) -> Option<NtfsVolume> {
    // The OEM name is the format's signature (partition type 0x07 is
    // shared with exFAT and HPFS, so the signature is what decides).
    if &sec[3..11] != b"NTFS    " {
        return None;
    }
    // v1 limit: 512-byte sectors (see module docs).
    if r16(sec, 11) != 512 {
        return None;
    }
    let spc_byte = sec[13];
    let sec_per_clus: u32 = if spc_byte == 0 {
        return None;
    } else if spc_byte < 0x80 {
        if !spc_byte.is_power_of_two() {
            return None;
        }
        spc_byte as u32
    } else {
        // Encoded form: cluster size is 2^(256 - value) bytes.
        let cluster_bytes = 1u32.checked_shl(256 - spc_byte as u32)?;
        if cluster_bytes < 512 || cluster_bytes % 512 != 0 {
            return None;
        }
        cluster_bytes / 512
    };
    let total_sectors = r64(sec, 40);
    if total_sectors == 0 {
        return None;
    }
    // Every LBA this volume can produce must fit the u32 sector callback.
    if total_sectors > u32::MAX as u64 {
        return None;
    }
    part_lba.checked_add(total_sectors as u32)?;
    let cluster_count = total_sectors / sec_per_clus as u64;
    if cluster_count == 0 {
        return None;
    }
    let mft_lcn = r64(sec, 48);
    if mft_lcn == 0 || mft_lcn >= cluster_count {
        return None;
    }
    let cluster_bytes = sec_per_clus * 512;
    let mft_record_size = decode_size_byte(sec[64], cluster_bytes)?;
    let index_record_size = decode_size_byte(sec[68], cluster_bytes)?;
    for size in [mft_record_size, index_record_size] {
        if size < 512 || size > MAX_RECORD_SIZE || size % 512 != 0 {
            return None;
        }
    }
    Some(NtfsVolume {
        part_lba,
        sec_per_clus,
        total_sectors,
        cluster_count,
        mft_lcn,
        mft_record_size,
        index_record_size,
        serial: r64(sec, 72),
        mft_data_len: 0,
        mft_runlist: [0u8; 256],
        mft_runlist_len: 0,
        label: [0u16; 32],
        label_len: 0,
    })
}

/// Validate and apply the update sequence (fixup) of one record in
/// place. The last two bytes of every sector must equal the update
/// sequence number; they are then replaced by the saved originals from
/// the sequence array. Any mismatch means the record was torn or is
/// corrupt, and it is rejected whole — a half-trusted record is worse
/// than none, because every offset inside it is attacker-controlled.
fn apply_fixup(rec: &mut [u8], magic: &[u8; 4]) -> bool {
    if rec.len() < 512 || rec.len() % 512 != 0 {
        return false;
    }
    if &rec[0..4] != magic {
        return false;
    }
    let usa_off = r16(rec, 4) as usize;
    let usa_cnt = r16(rec, 6) as usize;
    let sectors = rec.len() / 512;
    // The array holds the sequence number plus one saved word per sector.
    if usa_cnt != sectors + 1 {
        return false;
    }
    let Some(usa_end) = usa_off.checked_add(usa_cnt * 2) else {
        return false;
    };
    if usa_off < 16 || usa_end > rec.len() {
        return false;
    }
    let usn = r16(rec, usa_off);
    for i in 0..sectors {
        let tail = (i + 1) * 512 - 2;
        if r16(rec, tail) != usn {
            return false;
        }
        let saved = r16(rec, usa_off + 2 + i * 2);
        rec[tail] = saved as u8;
        rec[tail + 1] = (saved >> 8) as u8;
    }
    true
}

/// Offset of the first attribute in a FILE record, and the offset just
/// past the last one the record claims to use. Both are untrusted and
/// bounded against the buffer before any walk.
fn attr_walk_bounds(rec: &[u8]) -> Option<(usize, usize)> {
    if rec.len() < 0x20 {
        return None;
    }
    let first = r16(rec, 0x14) as usize;
    let used = r32(rec, 0x18) as usize;
    if first < 0x18 || first > used || used > rec.len() {
        return None;
    }
    Some((first, used))
}

/// Find an attribute by type. `want_unnamed` restricts to attributes
/// with no name (the unnamed $DATA is the file's content; named ones
/// are alternate streams). Returns the attribute's offset in `rec`.
fn find_attr(rec: &[u8], ty: u32, want_unnamed: bool) -> Option<usize> {
    let (mut off, used) = attr_walk_bounds(rec)?;
    loop {
        if off + 16 > used {
            return None;
        }
        let this_ty = r32(rec, off);
        if this_ty == 0xFFFF_FFFF {
            return None;
        }
        let len = r32(rec, off + 4) as usize;
        if len < 16 || off + len > rec.len() {
            return None; // corrupt length: stop, do not guess
        }
        if this_ty == ty && (!want_unnamed || rec[off + 9] == 0) {
            return Some(off);
        }
        off += len;
    }
}

/// The value bytes of a resident attribute, bounds-checked against the
/// attribute's own declared length.
fn attr_resident_value(rec: &[u8], off: usize) -> Option<&[u8]> {
    if rec[off + 8] != 0 {
        return None; // not resident
    }
    let attr_len = r32(rec, off + 4) as usize;
    let vlen = r32(rec, off + 16) as usize;
    let voff = r16(rec, off + 20) as usize;
    let end = voff.checked_add(vlen)?;
    if end > attr_len || off + end > rec.len() {
        return None;
    }
    Some(&rec[off + voff..off + end])
}

/// The parts of a non-resident attribute header this reader needs, as
/// plain values (no borrow of the record, so callers can keep using
/// their buffers while a stream is read).
#[derive(Copy, Clone)]
struct NonResident {
    runs_off: usize,
    runs_end: usize,
    data_len: u64,
    flags: u16,
}

fn attr_nonresident(rec: &[u8], off: usize) -> Option<NonResident> {
    if rec[off + 8] == 0 {
        return None; // resident
    }
    // A non-zero starting VCN marks a continuation segment reached via
    // $ATTRIBUTE_LIST, which v1 does not follow (module docs).
    if r64(rec, off + 16) != 0 {
        return None;
    }
    let attr_len = r32(rec, off + 4) as usize;
    let runs_off = off + r16(rec, off + 32) as usize;
    let runs_end = off + attr_len;
    if runs_off < off + 0x40 || runs_off > runs_end || runs_end > rec.len() {
        return None;
    }
    Some(NonResident {
        runs_off,
        runs_end,
        data_len: r64(rec, off + 48),
        flags: r16(rec, off + 12),
    })
}

/// One decoded runlist run: `len` clusters starting at stream VCN
/// `vcn`; `lcn` is the on-disk location, or `None` for a sparse run
/// (a hole, which reads as zeros).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct Run {
    vcn: u64,
    len: u64,
    lcn: Option<u64>,
}

/// Streaming runlist decoder. Runlists are delta-encoded: each run
/// carries a signed LCN delta from the previous run, so decoding is
/// inherently sequential and needs no storage — important because a
/// runlist can be long and this module does not allocate.
struct RunIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    vcn: u64,
    lcn: i64,
    bad: bool,
}

impl<'a> RunIter<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            vcn: 0,
            lcn: 0,
            bad: false,
        }
    }

    fn next_run(&mut self) -> Option<Run> {
        if self.bad || self.pos >= self.bytes.len() {
            return None;
        }
        let hdr = self.bytes[self.pos];
        if hdr == 0 {
            self.pos = self.bytes.len();
            return None;
        }
        let len_size = (hdr & 0x0F) as usize;
        let off_size = (hdr >> 4) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 {
            self.bad = true;
            return None;
        }
        let need = 1 + len_size + off_size;
        if self.pos + need > self.bytes.len() {
            self.bad = true;
            return None;
        }
        let mut length: u64 = 0;
        for i in 0..len_size {
            length |= (self.bytes[self.pos + 1 + i] as u64) << (8 * i);
        }
        if length == 0 {
            self.bad = true;
            return None;
        }
        let run_vcn = self.vcn;
        let lcn = if off_size == 0 {
            None // sparse run: no on-disk location, reads as zeros
        } else {
            let mut raw: i64 = 0;
            for i in 0..off_size {
                raw |= (self.bytes[self.pos + 1 + len_size + i] as i64) << (8 * i);
            }
            // Sign-extend from off_size bytes.
            let shift = 64 - 8 * off_size;
            let delta = (raw << shift) >> shift;
            let Some(next) = self.lcn.checked_add(delta) else {
                self.bad = true;
                return None;
            };
            if next < 0 {
                self.bad = true;
                return None;
            }
            self.lcn = next;
            Some(next as u64)
        };
        self.pos += need;
        self.vcn += length;
        Some(Run {
            vcn: run_vcn,
            len: length,
            lcn,
        })
    }
}

/// Read bytes from a runlist-addressed stream starting at byte offset
/// `start`, bounded by the stream's declared length `data_len`.
/// Returns the number of bytes actually read; a short read means the
/// stream ended, the runlist ran out, or the media errored.
fn read_runs<F>(
    vol: &NtfsVolume,
    runs: &[u8],
    data_len: u64,
    start: u64,
    out: &mut [u8],
    mut read_sec: F,
) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if start >= data_len || out.is_empty() {
        return 0;
    }
    let want = core::cmp::min(out.len() as u64, data_len - start) as usize;
    let cluster_bytes = vol.sec_per_clus as u64 * 512;
    let mut iter = RunIter::new(runs);
    let mut sec = [0u8; 512];
    let mut done = 0usize;
    let mut pos = start;
    while done < want {
        let Some(run) = iter.next_run() else {
            break;
        };
        // Runlist VCNs are untrusted: a corrupt length must not wrap
        // the byte range back into low addresses.
        let Some(run_start) = run.vcn.checked_mul(cluster_bytes) else {
            break;
        };
        let Some(run_end) = run
            .len
            .checked_mul(cluster_bytes)
            .and_then(|n| run_start.checked_add(n))
        else {
            break;
        };
        if run_end <= pos {
            continue;
        }
        if run_start > pos {
            break; // VCN gap: the runlist does not cover this range
        }
        while pos < run_end && done < want {
            match run.lcn {
                None => {
                    // Sparse: the hole reads as zeros, no media access.
                    // The fill must stop at the end of this run, not
                    // at the end of the request, or it would swallow
                    // the following real run's bytes as zeros.
                    let take = core::cmp::min(512, want - done)
                        .min((run_end - pos) as usize);
                    for b in &mut out[done..done + take] {
                        *b = 0;
                    }
                    done += take;
                    pos += take as u64;
                }
                Some(base_lcn) => {
                    let in_run = pos - run_start;
                    let cluster = base_lcn + in_run / cluster_bytes;
                    let in_cluster = in_run % cluster_bytes;
                    let lba = vol.part_lba as u64
                        + cluster * vol.sec_per_clus as u64
                        + in_cluster / 512;
                    if lba > u32::MAX as u64 {
                        return done;
                    }
                    if !read_sec(lba as u32, &mut sec) {
                        return done;
                    }
                    let off = (in_cluster % 512) as usize;
                    let take = core::cmp::min(512 - off, want - done);
                    out[done..done + take].copy_from_slice(&sec[off..off + take]);
                    done += take;
                    pos += take as u64;
                }
            }
        }
    }
    done
}

/// Read bytes of the $MFT itself (its $DATA stream) at byte offset `off`.
fn read_mft_bytes<F>(vol: &NtfsVolume, off: u64, out: &mut [u8], read_sec: F) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let len = vol.mft_runlist_len as usize;
    read_runs(vol, &vol.mft_runlist[..len], vol.mft_data_len, off, out, read_sec)
}

/// Load MFT record `rec_no` into `buf` and fixup-validate it. `buf`
/// must be at least the volume's MFT record size. Returns false for an
/// out-of-range record, a media error, or a record that fails fixup —
/// callers must not read fields from a record this rejected.
pub fn read_record<F>(vol: &NtfsVolume, rec_no: u64, buf: &mut [u8], mut read_sec: F) -> bool
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let size = vol.mft_record_size as usize;
    if buf.len() < size {
        return false;
    }
    let Some(off) = rec_no.checked_mul(size as u64) else {
        return false;
    };
    if read_mft_bytes(vol, off, &mut buf[..size], &mut read_sec) != size {
        return false;
    }
    apply_fixup(&mut buf[..size], b"FILE")
}

/// Whether a loaded record is in use. A record whose in-use flag is
/// clear is deleted (or never existed): it must be invisible to
/// lookups even if a stale index entry still points at it.
pub fn record_in_use(rec: &[u8]) -> bool {
    rec.len() >= 0x18 && r16(rec, 0x16) & 0x0001 != 0
}

/// Whether a loaded record describes a directory.
pub fn record_is_dir(rec: &[u8]) -> bool {
    rec.len() >= 0x18 && r16(rec, 0x16) & 0x0002 != 0
}

/// Namespace preference rank for $FILE_NAME: Win32 names are what a
/// user means, POSIX next, the 8.3 DOS alias last.
fn ns_rank(ns: u8) -> u8 {
    match ns {
        NS_WIN32 | NS_WIN32_DOS => 0,
        NS_POSIX => 1,
        NS_DOS => 2,
        _ => 3,
    }
}

/// Parse one $FILE_NAME attribute value into an entry. `record` is the
/// MFT record number the name belongs to (the value itself carries the
/// parent's reference, not its own).
fn entry_from_filename_value(v: &[u8], record: u64) -> Option<NtfsEntry> {
    if v.len() < 0x42 {
        return None;
    }
    let chars = v[0x40] as usize;
    if 0x42 + chars * 2 > v.len() {
        return None;
    }
    let mut e = NtfsEntry {
        name: [0u16; 128],
        name_len: 0,
        name_truncated: chars > 128,
        record,
        is_dir: r32(v, 0x38) & 0x1000_0000 != 0,
        data_len: r64(v, 0x30),
        modified: r64(v, 0x10),
    };
    let keep = core::cmp::min(chars, 128);
    for i in 0..keep {
        e.name[i] = r16(v, 0x42 + i * 2);
    }
    e.name_len = keep as u8;
    Some(e)
}

/// Build the entry for a loaded record from its best $FILE_NAME
/// (namespace preference Win32 > POSIX > DOS). The directory flag and
/// the $DATA size are then corrected from the record itself, which is
/// authoritative over the name attribute's cached copies.
pub fn record_entry(rec: &[u8], rec_no: u64) -> Option<NtfsEntry> {
    let (mut off, used) = attr_walk_bounds(rec)?;
    let mut best: Option<(u8, NtfsEntry)> = None;
    loop {
        if off + 16 > used {
            break;
        }
        let ty = r32(rec, off);
        if ty == 0xFFFF_FFFF {
            break;
        }
        let len = r32(rec, off + 4) as usize;
        if len < 16 || off + len > rec.len() {
            break;
        }
        if ty == 0x30 {
            if let Some(v) = attr_resident_value(rec, off) {
                if v.len() >= 0x42 {
                    let rank = ns_rank(v[0x41]);
                    if best.as_ref().map(|(r, _)| rank < *r).unwrap_or(true) {
                        if let Some(e) = entry_from_filename_value(v, rec_no) {
                            best = Some((rank, e));
                        }
                    }
                }
            }
        }
        off += len;
    }
    let mut e = best?.1;
    e.is_dir = record_is_dir(rec);
    // The unnamed $DATA header carries the authoritative stream size.
    if let Some(doff) = find_attr(rec, 0x80, true) {
        if rec[doff + 8] == 0 {
            if let Some(v) = attr_resident_value(rec, doff) {
                e.data_len = v.len() as u64;
            }
        } else if let Some(nr) = attr_nonresident(rec, doff) {
            e.data_len = nr.data_len;
        }
    }
    Some(e)
}

/// Read bytes of a loaded record's unnamed $DATA starting at `start`.
/// Resident data is copied straight from the record; non-resident data
/// is read through its runlist, with sparse runs returning zeros.
/// Compressed or encrypted streams return 0 (v1 refuses them).
pub fn read_data_at<F>(
    vol: &NtfsVolume,
    rec: &[u8],
    start: u64,
    out: &mut [u8],
    read_sec: F,
) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let Some(off) = find_attr(rec, 0x80, true) else {
        return 0;
    };
    if rec[off + 8] == 0 {
        let Some(v) = attr_resident_value(rec, off) else {
            return 0;
        };
        if start >= v.len() as u64 {
            return 0;
        }
        let n = core::cmp::min(out.len(), v.len() - start as usize);
        out[..n].copy_from_slice(&v[start as usize..start as usize + n]);
        n
    } else {
        let Some(nr) = attr_nonresident(rec, off) else {
            return 0;
        };
        // Compression (0x0001) and encryption (0x4000) need transforms
        // v1 does not implement; the compression-unit field is a second
        // witness of the compressed case.
        if nr.flags & 0x4001 != 0 || r16(rec, off + 34) != 0 {
            return 0;
        }
        let runs = &rec[nr.runs_off..nr.runs_end];
        read_runs(vol, runs, nr.data_len, start, out, read_sec)
    }
}

/// Read file data for an entry: loads its record into `rec_buf`
/// (fixup-validated), refuses deleted records, then reads the unnamed
/// $DATA. This is the read path the diagnostic tool drives.
pub fn read_entry_data<F>(
    vol: &NtfsVolume,
    entry: &NtfsEntry,
    start: u64,
    out: &mut [u8],
    rec_buf: &mut [u8],
    mut read_sec: F,
) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if !read_record(vol, entry.record, rec_buf, &mut read_sec) {
        return 0;
    }
    let size = vol.mft_record_size as usize;
    if !record_in_use(&rec_buf[..size]) {
        return 0;
    }
    read_data_at(vol, &rec_buf[..size], start, out, read_sec)
}

/// Case-fold one character for name comparison: the built-in subset
/// described in the module docs (ASCII exactly, Latin-1 letters by the
/// Unicode simple mapping, everything else identity).
fn upcase_char(ch: u16) -> u16 {
    match ch {
        0x61..=0x7A => ch - 0x20,              // a-z
        0xE0..=0xFE if ch != 0xF7 => ch - 0x20, // à-þ except ÷
        0xFF => 0x178,                          // ÿ -> Ÿ
        _ => ch,
    }
}

fn name_eq_ci(a: &[u16], b: &[u16]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if upcase_char(a[i]) != upcase_char(b[i]) {
            return false;
        }
    }
    true
}

/// Scan one index entry area (from $INDEX_ROOT or an INDX block),
/// handing each live entry to `cb` and collecting sub-node VCNs into
/// `pending` for the caller to visit. Sets `*stop` when `cb` asks to
/// halt. Entries are variable-sized and entirely untrusted: a bad size
/// ends the scan of this block rather than guessing.
fn scan_index_entries<C>(
    bytes: &[u8],
    pending: &mut [u64; 64],
    np: &mut usize,
    cb: &mut C,
    stop: &mut bool,
) where
    C: FnMut(&NtfsEntry) -> bool,
{
    let mut off = 0usize;
    while off + 16 <= bytes.len() {
        let size = r16(bytes, off + 8) as usize;
        if size < 16 || off + size > bytes.len() {
            return;
        }
        let flags = r16(bytes, off + 12);
        let key_len = r16(bytes, off + 10) as usize;
        let is_last = flags & 0x0002 != 0;
        if !is_last && 16 + key_len <= size {
            let record = r64(bytes, off) & 0x0000_FFFF_FFFF_FFFF;
            if let Some(e) = entry_from_filename_value(&bytes[off + 16..off + 16 + key_len], record)
            {
                if !cb(&e) {
                    *stop = true;
                }
            }
        }
        if flags & 0x0001 != 0 {
            // Sub-node: the child block's VCN is the entry's last 8
            // bytes, in cluster units within $INDEX_ALLOCATION.
            if size >= 8 {
                let vcn = r64(bytes, off + size - 8);
                if *np < pending.len() {
                    pending[*np] = vcn;
                    *np += 1;
                }
            }
        }
        if is_last || *stop {
            return;
        }
        off += size;
    }
}

/// Locate a directory record's filename $INDEX_ROOT value (the index
/// whose indexed attribute type is $FILE_NAME, 0x30 — directories can
/// carry other indexes, and only this one lists files).
fn find_filename_index_root(rec: &[u8]) -> Option<usize> {
    let (mut off, used) = attr_walk_bounds(rec)?;
    loop {
        if off + 16 > used {
            return None;
        }
        let ty = r32(rec, off);
        if ty == 0xFFFF_FFFF {
            return None;
        }
        let len = r32(rec, off + 4) as usize;
        if len < 16 || off + len > rec.len() {
            return None;
        }
        if ty == 0x90 {
            if let Some(v) = attr_resident_value(rec, off) {
                if v.len() >= 16 && r32(v, 0) == 0x30 {
                    return Some(off);
                }
            }
        }
        off += len;
    }
}

/// Iterate the entries of directory record `dir_record`, walking the
/// $INDEX_ROOT entries and every $INDEX_ALLOCATION sub-node they point
/// at. `rec_buf` holds the directory record; `indx_buf` (at least the
/// index record size) holds INDX blocks, each fixup-validated and
/// checked against the VCN it was fetched by. Returning false from
/// `cb` stops the walk.
pub fn for_each_dir_entry<F, C>(
    vol: &NtfsVolume,
    dir_record: u64,
    rec_buf: &mut [u8],
    indx_buf: &mut [u8],
    mut read_sec: F,
    mut cb: C,
) where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
    C: FnMut(&NtfsEntry) -> bool,
{
    let rec_size = vol.mft_record_size as usize;
    let idx_size = vol.index_record_size as usize;
    if indx_buf.len() < idx_size {
        return;
    }
    if !read_record(vol, dir_record, rec_buf, &mut read_sec) {
        return;
    }
    if !record_in_use(&rec_buf[..rec_size]) || !record_is_dir(&rec_buf[..rec_size]) {
        return;
    }
    let Some(root_off) = find_filename_index_root(&rec_buf[..rec_size]) else {
        return;
    };
    // Extract the root's entry area (as offsets within the attribute
    // value) and the allocation's runlist location as plain values
    // before scanning, so the INDX loads below work with data, not
    // borrows of a buffer they might share the call stack with.
    let (entries_start, entries_end, alloc) = {
        let rec = &rec_buf[..rec_size];
        let Some(v) = attr_resident_value(rec, root_off) else {
            return;
        };
        if v.len() < 32 {
            return;
        }
        // Index header sits 16 bytes into the value; its entry offsets
        // are relative to the index header itself.
        let rel_off = r32(v, 16) as usize;
        let rel_used = r32(v, 20) as usize;
        if rel_off < 16 || rel_used < rel_off || 16 + rel_used > v.len() {
            return;
        }
        let alloc = find_attr(rec, 0xA0, false).and_then(|o| attr_nonresident(rec, o));
        (16 + rel_off, 16 + rel_used, alloc)
    };
    let mut pending = [0u64; 64];
    let mut np = 0usize;
    let mut stop = false;
    {
        let rec = &rec_buf[..rec_size];
        let Some(v) = attr_resident_value(rec, root_off) else {
            return;
        };
        scan_index_entries(
            &v[entries_start..entries_end],
            &mut pending,
            &mut np,
            &mut cb,
            &mut stop,
        );
    }
    let cluster_bytes = vol.sec_per_clus as u64 * 512;
    let mut visited = 0u32;
    while np > 0 && !stop {
        np -= 1;
        let vcn = pending[np];
        visited += 1;
        if visited > 4096 {
            break; // cyclic or corrupt tree: bounded, never spinning
        }
        let Some(a) = alloc else {
            break;
        };
        let Some(byte_off) = vcn.checked_mul(cluster_bytes) else {
            continue;
        };
        let runs = &rec_buf[a.runs_off..a.runs_end];
        let n = read_runs(
            vol,
            runs,
            a.data_len,
            byte_off,
            &mut indx_buf[..idx_size],
            &mut read_sec,
        );
        if n != idx_size {
            continue;
        }
        if !apply_fixup(&mut indx_buf[..idx_size], b"INDX") {
            continue;
        }
        // The block must be the block we asked for: its header carries
        // its own VCN, and a mismatch means the tree is inconsistent.
        if r64(&indx_buf, 0x10) != vcn {
            continue;
        }
        let rel_off = r32(&indx_buf, 0x18) as usize;
        let rel_used = r32(&indx_buf, 0x1C) as usize;
        if rel_off < 16 || rel_used < rel_off || 0x18 + rel_used > idx_size {
            continue;
        }
        scan_index_entries(
            &indx_buf[0x18 + rel_off..0x18 + rel_used],
            &mut pending,
            &mut np,
            &mut cb,
            &mut stop,
        );
    }
}

/// Look up a path (components separated by `\` or `/`, as UTF-16 code
/// units) starting from the root directory. Each component is found in
/// the containing directory's index, then verified against the file's
/// own record: the returned entry carries the record's preferred
/// $FILE_NAME and authoritative $DATA size, and a deleted record is
/// never returned.
pub fn find_path<F>(
    vol: &NtfsVolume,
    path: &[u16],
    rec_buf: &mut [u8],
    indx_buf: &mut [u8],
    mut read_sec: F,
) -> Option<NtfsEntry>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut cur_dir = ROOT_RECORD;
    let mut rest = path;
    while !rest.is_empty() && (rest[0] == b'\\' as u16 || rest[0] == b'/' as u16) {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return None;
    }
    let rec_size = vol.mft_record_size as usize;
    loop {
        let mut sep = rest.len();
        for i in 0..rest.len() {
            if rest[i] == b'\\' as u16 || rest[i] == b'/' as u16 {
                sep = i;
                break;
            }
        }
        let comp = &rest[..sep];
        if comp.is_empty() || comp.len() > 128 {
            return None;
        }
        let mut hit: Option<u64> = None;
        for_each_dir_entry(
            vol,
            cur_dir,
            rec_buf,
            indx_buf,
            &mut read_sec,
            |e| {
                if e.name_truncated || e.name_len as usize != comp.len() {
                    return true;
                }
                if name_eq_ci(&e.name[..comp.len()], comp) {
                    hit = Some(e.record);
                    return false;
                }
                true
            },
        );
        let rec_no = hit?;
        if !read_record(vol, rec_no, rec_buf, &mut read_sec) {
            return None;
        }
        if !record_in_use(&rec_buf[..rec_size]) {
            return None; // deleted: invisible even if the index is stale
        }
        let entry = record_entry(&rec_buf[..rec_size], rec_no)?;
        if sep >= rest.len() {
            return Some(entry);
        }
        if !entry.is_dir {
            return None;
        }
        cur_dir = rec_no;
        rest = &rest[sep + 1..];
        while !rest.is_empty() && (rest[0] == b'\\' as u16 || rest[0] == b'/' as u16) {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return Some(entry);
        }
    }
}

/// Count free clusters from $Bitmap (MFT record 6): a clear bit is a
/// free cluster. Streamed in sector chunks so no bitmap-sized buffer
/// is ever needed.
pub fn free_clusters<F>(vol: &NtfsVolume, rec_buf: &mut [u8], mut read_sec: F) -> Option<u64>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let rec_size = vol.mft_record_size as usize;
    if !read_record(vol, BITMAP_RECORD, rec_buf, &mut read_sec) {
        return None;
    }
    if !record_in_use(&rec_buf[..rec_size]) {
        return None;
    }
    let mut free = 0u64;
    let mut off = 0u64;
    let mut chunk = [0u8; 512];
    loop {
        let n = read_data_at(vol, &rec_buf[..rec_size], off, &mut chunk, &mut read_sec);
        if n == 0 {
            break;
        }
        for i in 0..n {
            let base = (off as usize + i) * 8;
            if base as u64 >= vol.cluster_count {
                break;
            }
            for bit in 0..8usize {
                if base as u64 + bit as u64 >= vol.cluster_count {
                    break;
                }
                if chunk[i] & (1 << bit) == 0 {
                    free += 1;
                }
            }
        }
        off += n as u64;
        if off * 8 >= vol.cluster_count {
            break;
        }
    }
    Some(free)
}

/// Inspect sector 0: if it is an NTFS boot sector (superfloppy), mount
/// directly; otherwise scan MBR partitions of type 0x07 (shared with
/// exFAT/HPFS — the OEM signature decides) for the first NTFS volume.
/// Mounting proves the volume against itself: $MFT record 0 must
/// fixup-validate and yield its runlist, and the root directory record
/// must load through that runlist carrying a filename index. The
/// volume label is read from $Volume when present. `rec_buf` (at
/// least the MFT record size) is caller-supplied scratch — see the
/// module docs for why no record buffer is ever a local here.
pub fn mount_volume<F>(rec_buf: &mut [u8], mut read_sec: F) -> Option<NtfsVolume>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut sec0 = [0u8; 512];
    if !read_sec(0, &mut sec0) {
        return None;
    }
    let mut candidates = [0u32; 5];
    let mut ncand = 0usize;
    if sec0[510] == 0x55 && sec0[511] == 0xAA {
        for i in 0..4usize {
            let p = 446 + i * 16;
            let ptype = sec0[p + 4];
            let start_lba = r32(&sec0, p + 8);
            let num_secs = r32(&sec0, p + 12);
            if ptype == 0x07 && start_lba > 0 && num_secs > 0 {
                candidates[ncand] = start_lba;
                ncand += 1;
            }
        }
    }
    candidates[ncand] = 0;
    ncand += 1;

    for &part_lba in &candidates[..ncand] {
        let mut vbr = [0u8; 512];
        if !read_sec(part_lba, &mut vbr) {
            continue;
        }
        let Some(mut vol) = parse_boot(&vbr, part_lba) else {
            continue;
        };
        let rec_size = vol.mft_record_size as usize;
        if rec_buf.len() < rec_size {
            continue;
        }
        // Bootstrap: record 0 sits at the head of $MFT, whose first run
        // begins at the boot sector's $MFT LCN by definition, so it can
        // be read directly before any runlist is known. Sectors move
        // through a 512-byte scratch, never a record-sized local.
        let mft_start = part_lba as u64 + vol.mft_lcn * vol.sec_per_clus as u64;
        let mut sec = [0u8; 512];
        let mut ok = true;
        for s in 0..rec_size / 512 {
            let lba = mft_start + s as u64;
            if lba > u32::MAX as u64 || !read_sec(lba as u32, &mut sec) {
                ok = false;
                break;
            }
            rec_buf[s * 512..(s + 1) * 512].copy_from_slice(&sec);
        }
        if !ok || !apply_fixup(&mut rec_buf[..rec_size], b"FILE") {
            continue;
        }
        if !record_in_use(&rec_buf[..rec_size]) {
            continue;
        }
        let Some(mft_data_off) = find_attr(&rec_buf[..rec_size], 0x80, true) else {
            continue;
        };
        let Some(nr) = attr_nonresident(&rec_buf[..rec_size], mft_data_off) else {
            continue;
        };
        let runs_len = nr.runs_end - nr.runs_off;
        if runs_len > vol.mft_runlist.len() {
            continue; // $MFT runlist beyond the v1 bound (module docs)
        }
        vol.mft_runlist[..runs_len]
            .copy_from_slice(&rec_buf[nr.runs_off..nr.runs_end]);
        vol.mft_runlist_len = runs_len as u16;
        vol.mft_data_len = nr.data_len;
        // The root directory must load through the captured runlist and
        // carry a filename index, or this is not a volume we can serve.
        if !read_record(&vol, ROOT_RECORD, rec_buf, &mut read_sec) {
            continue;
        }
        if !record_in_use(&rec_buf[..rec_size])
            || !record_is_dir(&rec_buf[..rec_size])
            || find_filename_index_root(&rec_buf[..rec_size]).is_none()
        {
            continue;
        }
        // Label from $Volume's $VOLUME_NAME, when the record is there.
        if read_record(&vol, VOLUME_RECORD, rec_buf, &mut read_sec)
            && record_in_use(&rec_buf[..rec_size])
        {
            if let Some(off) = find_attr(&rec_buf[..rec_size], 0x60, true) {
                if let Some(v) = attr_resident_value(&rec_buf[..rec_size], off) {
                    let chars = core::cmp::min(v.len() / 2, 32);
                    for i in 0..chars {
                        vol.label[i] = r16(v, i * 2);
                    }
                    vol.label_len = chars as u8;
                }
            }
        }
        return Some(vol);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic NTFS image, built byte by byte with genuinely applied
    // fixups: 512-byte sectors and clusters, 1024-byte MFT records,
    // 4096-byte index blocks, 400 clusters total.
    //
    // Cluster map: 0 boot, 16..79 $MFT (records 0..31), 100 $Bitmap
    // data, 120..123 BIG.BIN run A, 130..133 BIG.BIN run B,
    // 200..215 root $INDEX_ALLOCATION (two INDX blocks).
    //
    // Records: 0 $MFT, 3 $Volume (label NTFSTEST), 5 root, 6 $Bitmap,
    // 16 HELLO.TXT (resident), 17 BIG.BIN (non-resident, sparse gap),
    // 18 GAMES (dir), 19 DOOM.TXT (resident, in GAMES), 20 the long
    // name (Win32 + DOS $FILE_NAMEs), 21 GONE.TXT (deleted; a stale
    // root index entry still points at it).
    const TOTAL_CLUSTERS: u64 = 400;
    const MFT_LCN: u64 = 16;
    const BITMAP_LCN: u64 = 100;
    const BIG_A_LCN: u64 = 120;
    const BIG_B_LCN: u64 = 130;
    const INDX_LCN: u64 = 200;
    const SERIAL: u64 = 0x2026_1124;

    const HELLO_PAYLOAD: &[u8] = b"FERRIC-NTFS-RO-PASS-2026\r\n";
    const DOOM_PAYLOAD: &[u8] = b"DOOM in a subdirectory, over NTFS.\r\n";
    const LONG_PAYLOAD: &[u8] = b"A name long enough to need two namespaces.\r\n";
    const LONG_NAME: &str = "LongFileNameTest.txt";
    const DOS_NAME: &str = "LONGFI~1.TXT";

    fn big_payload() -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..5120u32 {
            if (2048..3072).contains(&i) {
                v.push(0);
            } else {
                v.push(((i * 7 + 3) & 0xFF) as u8);
            }
        }
        v
    }

    fn w16(d: &mut [u8], off: usize, v: u16) {
        d[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn w32(d: &mut [u8], off: usize, v: u32) {
        d[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn w64(d: &mut [u8], off: usize, v: u64) {
        d[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn align8(n: usize) -> usize {
        (n + 7) & !7
    }

    /// Apply a genuine update sequence to a record image: the current
    /// last-two-bytes of each sector are saved into the USA and the
    /// sequence number takes their place, exactly as a formatter
    /// would leave the record on disk.
    fn apply_usa(rec: &mut [u8], usa_off: usize) {
        let sectors = rec.len() / 512;
        w16(rec, usa_off, 0x4E54); // "NT": arbitrary sequence number
        for i in 0..sectors {
            let tail = (i + 1) * 512 - 2;
            let saved = r16(rec, tail);
            w16(rec, usa_off + 2 + i * 2, saved);
            w16(rec, tail, 0x4E54);
        }
    }

    fn attr_resident(ty: u32, name: Option<&str>, value: &[u8]) -> Vec<u8> {
        let name16: Vec<u16> = name.map(|s| s.encode_utf16().collect()).unwrap_or_default();
        let voff = align8(24 + name16.len() * 2);
        let mut a = vec![0u8; align8(voff + value.len())];
        let alen = a.len() as u32;
        w32(&mut a, 0, ty);
        w32(&mut a, 4, alen);
        a[9] = name16.len() as u8;
        w16(&mut a, 10, 24);
        for (i, &ch) in name16.iter().enumerate() {
            w16(&mut a, 24 + i * 2, ch);
        }
        w32(&mut a, 16, value.len() as u32);
        w16(&mut a, 20, voff as u16);
        a[voff..voff + value.len()].copy_from_slice(value);
        a
    }

    /// Encode a runlist from (length, Option<lcn_delta>) pairs.
    fn runlist(runs: &[(u64, Option<i64>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(len, delta) in runs {
            let mut lb = [0u8; 8];
            let mut n = 0usize;
            let mut v = len;
            while v > 0 {
                lb[n] = (v & 0xFF) as u8;
                v >>= 8;
                n += 1;
            }
            let len_size = core::cmp::max(n, 1);
            match delta {
                None => {
                    out.push(len_size as u8);
                    out.extend_from_slice(&lb[..len_size]);
                }
                Some(d) => {
                    // Smallest signed byte count that holds the delta.
                    let mut ob = [0u8; 8];
                    let mut m = 1usize;
                    loop {
                        let min = -(1i64 << (8 * m - 1));
                        let max = (1i64 << (8 * m - 1)) - 1;
                        if d >= min && d <= max {
                            break;
                        }
                        m += 1;
                    }
                    let uv = d as u64;
                    for i in 0..m {
                        ob[i] = (uv >> (8 * i)) as u8;
                    }
                    out.push(((m as u8) << 4) | len_size as u8);
                    out.extend_from_slice(&lb[..len_size]);
                    out.extend_from_slice(&ob[..m]);
                }
            }
        }
        out.push(0);
        out
    }

    fn attr_data_nonresident(runs: &[(u64, Option<i64>)], data_len: u64) -> Vec<u8> {
        attr_nonresident_typed(0x80, runs, data_len)
    }

    fn attr_nonresident_typed(ty: u32, runs: &[(u64, Option<i64>)], data_len: u64) -> Vec<u8> {
        let rl = runlist(runs);
        let total_clusters: u64 = runs.iter().map(|r| r.0).sum();
        let mut a = vec![0u8; align8(64 + rl.len())];
        let alen = a.len() as u32;
        w32(&mut a, 0, ty);
        w32(&mut a, 4, alen);
        a[8] = 1; // non-resident
        w64(&mut a, 24, total_clusters - 1); // last VCN
        w16(&mut a, 32, 64); // runlist offset
        w64(&mut a, 40, total_clusters * 512); // allocated size
        w64(&mut a, 48, data_len);
        w64(&mut a, 56, data_len); // initialized size
        a[64..64 + rl.len()].copy_from_slice(&rl);
        a
    }

    fn filename_value(name: &str, ns: u8, parent: u64, is_dir: bool, data_len: u64) -> Vec<u8> {
        let name16: Vec<u16> = name.encode_utf16().collect();
        let mut v = vec![0u8; 0x42 + name16.len() * 2];
        w64(&mut v, 0, parent | (1 << 48));
        w64(&mut v, 0x10, 0x01DC_0000_0000_0000); // fixed FILETIME
        w64(&mut v, 0x28, data_len);
        w64(&mut v, 0x30, data_len);
        w32(&mut v, 0x38, if is_dir { 0x1000_0000 } else { 0x20 });
        v[0x40] = name16.len() as u8;
        v[0x41] = ns;
        for (i, &ch) in name16.iter().enumerate() {
            w16(&mut v, 0x42 + i * 2, ch);
        }
        v
    }

    fn filename_attr(name: &str, ns: u8, parent: u64, is_dir: bool, data_len: u64) -> Vec<u8> {
        attr_resident(0x30, None, &filename_value(name, ns, parent, is_dir, data_len))
    }

    /// One index entry: file reference + $FILE_NAME key, optionally
    /// carrying a sub-node VCN in its last 8 bytes.
    fn index_entry(
        record: u64,
        name: &str,
        ns: u8,
        parent: u64,
        is_dir: bool,
        data_len: u64,
        subnode: Option<u64>,
    ) -> Vec<u8> {
        let key = filename_value(name, ns, parent, is_dir, data_len);
        let body = 16 + key.len();
        let size = align8(body + if subnode.is_some() { 8 } else { 0 });
        let mut e = vec![0u8; size];
        w64(&mut e, 0, record | (1 << 48));
        w16(&mut e, 8, size as u16);
        w16(&mut e, 10, key.len() as u16);
        w16(&mut e, 12, if subnode.is_some() { 0x0001 } else { 0 });
        e[16..16 + key.len()].copy_from_slice(&key);
        if let Some(vcn) = subnode {
            w64(&mut e, size - 8, vcn);
        }
        e
    }

    fn index_end(subnode: Option<u64>) -> Vec<u8> {
        let size = if subnode.is_some() { 24 } else { 16 };
        let mut e = vec![0u8; size];
        w16(&mut e, 8, size as u16);
        w16(&mut e, 12, 0x0002 | if subnode.is_some() { 0x0001 } else { 0 });
        if let Some(vcn) = subnode {
            w64(&mut e, size - 8, vcn);
        }
        e
    }

    fn index_root_attr(entries: &[u8], large: bool) -> Vec<u8> {
        let mut v = vec![0u8; 32 + entries.len()];
        w32(&mut v, 0, 0x30); // indexes $FILE_NAME
        w32(&mut v, 4, 1); // collation: filename
        w32(&mut v, 8, 4096); // index block size
        v[12] = 8; // clusters per index block (512-byte clusters)
        w32(&mut v, 16, 16); // entries offset, rel. to index header
        w32(&mut v, 20, (16 + entries.len()) as u32);
        w32(&mut v, 24, (16 + entries.len()) as u32);
        w32(&mut v, 28, if large { 1 } else { 0 });
        v[32..].copy_from_slice(entries);
        attr_resident(0x90, Some("$I30"), &v)
    }

    fn file_record(rec_no: u32, flags: u16, attrs: &[u8]) -> Vec<u8> {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        w16(&mut rec, 4, 0x30); // USA offset
        w16(&mut rec, 6, 3); // USA count: 2 sectors + 1
        w16(&mut rec, 0x10, 1); // sequence
        w16(&mut rec, 0x12, 1); // link count
        let first = 0x38usize;
        w16(&mut rec, 0x14, first as u16);
        w16(&mut rec, 0x16, flags);
        let used = first + attrs.len() + 8;
        w32(&mut rec, 0x18, used as u32);
        w32(&mut rec, 0x1C, 1024);
        w32(&mut rec, 0x2C, rec_no);
        rec[first..first + attrs.len()].copy_from_slice(attrs);
        w32(&mut rec, first + attrs.len(), 0xFFFF_FFFF); // end marker
        apply_usa(&mut rec, 0x30);
        rec
    }

    fn indx_block(vcn: u64, entries: &[u8]) -> Vec<u8> {
        let mut rec = vec![0u8; 4096];
        rec[0..4].copy_from_slice(b"INDX");
        w16(&mut rec, 4, 0x28); // USA offset
        w16(&mut rec, 6, 9); // USA count: 8 sectors + 1
        w64(&mut rec, 0x10, vcn);
        let entries_at = 0x40usize;
        w32(&mut rec, 0x18, (entries_at - 0x18) as u32);
        w32(&mut rec, 0x1C, (entries_at - 0x18 + entries.len()) as u32);
        w32(&mut rec, 0x20, 4096 - 0x18);
        rec[entries_at..entries_at + entries.len()].copy_from_slice(entries);
        apply_usa(&mut rec, 0x28);
        rec
    }

    fn boot_sector() -> [u8; 512] {
        let mut s = [0u8; 512];
        s[0] = 0xEB;
        s[1] = 0x52;
        s[2] = 0x90;
        s[3..11].copy_from_slice(b"NTFS    ");
        w16(&mut s, 11, 512);
        s[13] = 1; // one sector per cluster
        s[21] = 0xF8; // media
        w64(&mut s, 40, TOTAL_CLUSTERS); // total sectors
        w64(&mut s, 48, MFT_LCN);
        w64(&mut s, 56, MFT_LCN); // $MFTMirr (same place is fine here)
        s[64] = (-10i8) as u8; // MFT record = 2^10 bytes
        s[68] = (-12i8) as u8; // index block = 2^12 bytes
        w64(&mut s, 72, SERIAL);
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    /// Assemble the whole image with the partition at sector `base`
    /// (0 = superfloppy; otherwise an MBR is written pointing there).
    fn build_image(base: u32) -> Vec<u8> {
        let mut img = vec![0u8; (base as usize + TOTAL_CLUSTERS as usize) * 512];
        let at = |clus: u64| (base as usize + clus as usize) * 512;
        img[at(0)..at(0) + 512].copy_from_slice(&boot_sector());

        // $Bitmap contents: mark the clusters this layout uses.
        let mut bitmap = [0u8; 512];
        let mark = |clus: u64, bitmap: &mut [u8; 512]| {
            bitmap[(clus / 8) as usize] |= 1 << (clus % 8);
        };
        mark(0, &mut bitmap);
        for c in MFT_LCN..MFT_LCN + 64 {
            mark(c, &mut bitmap);
        }
        mark(BITMAP_LCN, &mut bitmap);
        for c in BIG_A_LCN..BIG_A_LCN + 4 {
            mark(c, &mut bitmap);
        }
        for c in BIG_B_LCN..BIG_B_LCN + 4 {
            mark(c, &mut bitmap);
        }
        for c in INDX_LCN..INDX_LCN + 16 {
            mark(c, &mut bitmap);
        }
        img[at(BITMAP_LCN)..at(BITMAP_LCN) + 512].copy_from_slice(&bitmap);

        // BIG.BIN data: run A holds payload[0..2048], run B [3072..5120].
        let big = big_payload();
        img[at(BIG_A_LCN)..at(BIG_A_LCN) + 2048].copy_from_slice(&big[0..2048]);
        img[at(BIG_B_LCN)..at(BIG_B_LCN) + 2048].copy_from_slice(&big[3072..5120]);

        // Root index blocks. VCNs are cluster units within the
        // allocation stream: block 0 at stream offset 0 (VCN 0),
        // block 1 at offset 4096 (VCN 8).
        let mut blk0 = Vec::new(); // keys above HELLO.TXT
        blk0.extend_from_slice(&index_entry(20, LONG_NAME, NS_WIN32, 5, false, LONG_PAYLOAD.len() as u64, None));
        blk0.extend_from_slice(&index_entry(20, DOS_NAME, NS_DOS, 5, false, LONG_PAYLOAD.len() as u64, None));
        blk0.extend_from_slice(&index_end(None));
        let blk0 = indx_block(0, &blk0);
        img[at(INDX_LCN)..at(INDX_LCN) + 4096].copy_from_slice(&blk0);
        let mut blk1 = Vec::new(); // keys below HELLO.TXT
        blk1.extend_from_slice(&index_entry(17, "BIG.BIN", NS_WIN32, 5, false, 5120, None));
        blk1.extend_from_slice(&index_entry(21, "GONE.TXT", NS_WIN32, 5, false, 4, None));
        blk1.extend_from_slice(&index_end(None));
        let blk1 = indx_block(8, &blk1);
        img[at(INDX_LCN + 8)..at(INDX_LCN + 8) + 4096].copy_from_slice(&blk1);

        // ---- MFT records
        let put_rec = |img: &mut Vec<u8>, rec_no: u32, rec: &[u8]| {
            let off = at(MFT_LCN) + rec_no as usize * 1024;
            img[off..off + 1024].copy_from_slice(rec);
        };

        // 0: $MFT — its $DATA runlist is the mount bootstrap.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("$MFT", NS_WIN32, 5, false, 64 * 512));
        attrs.extend_from_slice(&attr_data_nonresident(&[(64, Some(MFT_LCN as i64))], 64 * 512));
        put_rec(&mut img, 0, &file_record(0, 0x0001, &attrs));

        // 3: $Volume — carries the label.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("$Volume", NS_WIN32, 5, false, 0));
        let label16: Vec<u16> = "NTFSTEST".encode_utf16().collect();
        let mut label_bytes = Vec::new();
        for ch in label16 {
            label_bytes.extend_from_slice(&ch.to_le_bytes());
        }
        attrs.extend_from_slice(&attr_resident(0x60, None, &label_bytes));
        put_rec(&mut img, 3, &file_record(3, 0x0001, &attrs));

        // 5: root directory — index root plus allocation.
        let mut root_entries = Vec::new();
        root_entries.extend_from_slice(&index_entry(18, "GAMES", NS_WIN32, 5, true, 0, None));
        root_entries.extend_from_slice(&index_entry(
            16,
            "HELLO.TXT",
            NS_WIN32,
            5,
            false,
            HELLO_PAYLOAD.len() as u64,
            Some(8), // sub-node: BIG.BIN + GONE.TXT block
        ));
        root_entries.extend_from_slice(&index_end(Some(0))); // sub-node: long-name block
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr(".", NS_WIN32, 5, true, 0));
        attrs.extend_from_slice(&index_root_attr(&root_entries, true));
        attrs.extend_from_slice(&attr_nonresident_typed(0xA0, &[(16, Some(INDX_LCN as i64))], 16 * 512));
        put_rec(&mut img, 5, &file_record(5, 0x0003, &attrs));

        // 6: $Bitmap.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("$Bitmap", NS_WIN32, 5, false, 512));
        attrs.extend_from_slice(&attr_data_nonresident(&[(1, Some(BITMAP_LCN as i64))], 512));
        put_rec(&mut img, 6, &file_record(6, 0x0001, &attrs));

        // 16: HELLO.TXT, resident.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("HELLO.TXT", NS_WIN32, 5, false, HELLO_PAYLOAD.len() as u64));
        attrs.extend_from_slice(&attr_resident(0x80, None, HELLO_PAYLOAD));
        put_rec(&mut img, 16, &file_record(16, 0x0001, &attrs));

        // 17: BIG.BIN, non-resident with a sparse gap.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("BIG.BIN", NS_WIN32, 5, false, 5120));
        // Runlist deltas are relative to the previous run's starting
        // LCN (sparse runs do not move the accumulator), so run B's
        // delta is 130 - 120, not 130 - 124.
        attrs.extend_from_slice(&attr_data_nonresident(
            &[
                (4, Some(BIG_A_LCN as i64)),
                (2, None),
                (4, Some((BIG_B_LCN - BIG_A_LCN) as i64)),
            ],
            5120,
        ));
        put_rec(&mut img, 17, &file_record(17, 0x0001, &attrs));

        // 18: GAMES directory with DOOM.TXT in its index root.
        let mut games_entries = Vec::new();
        games_entries.extend_from_slice(&index_entry(19, "DOOM.TXT", NS_WIN32, 18, false, DOOM_PAYLOAD.len() as u64, None));
        games_entries.extend_from_slice(&index_end(None));
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("GAMES", NS_WIN32, 5, true, 0));
        attrs.extend_from_slice(&index_root_attr(&games_entries, false));
        put_rec(&mut img, 18, &file_record(18, 0x0003, &attrs));

        // 19: DOOM.TXT, resident.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("DOOM.TXT", NS_WIN32, 18, false, DOOM_PAYLOAD.len() as u64));
        attrs.extend_from_slice(&attr_resident(0x80, None, DOOM_PAYLOAD));
        put_rec(&mut img, 19, &file_record(19, 0x0001, &attrs));

        // 20: the long-named file, Win32 + DOS names, resident data.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr(LONG_NAME, NS_WIN32, 5, false, LONG_PAYLOAD.len() as u64));
        attrs.extend_from_slice(&filename_attr(DOS_NAME, NS_DOS, 5, false, LONG_PAYLOAD.len() as u64));
        attrs.extend_from_slice(&attr_resident(0x80, None, LONG_PAYLOAD));
        put_rec(&mut img, 20, &file_record(20, 0x0001, &attrs));

        // 21: GONE.TXT — a complete record whose in-use flag is clear.
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&filename_attr("GONE.TXT", NS_WIN32, 5, false, 4));
        attrs.extend_from_slice(&attr_resident(0x80, None, b"GONE"));
        put_rec(&mut img, 21, &file_record(21, 0x0000, &attrs));

        if base > 0 {
            // MBR: one type-0x07 partition covering the NTFS volume.
            img[510] = 0x55;
            img[511] = 0xAA;
            let p = 446;
            img[p] = 0x80;
            img[p + 4] = 0x07;
            w32(&mut img, p + 8, base);
            w32(&mut img, p + 12, TOTAL_CLUSTERS as u32);
        }
        img
    }

    fn mount(img: &[u8]) -> Option<(NtfsVolume, [u8; 4096], [u8; 4096])> {
        let mut rec_buf = [0u8; 4096];
        let vol = mount_volume(&mut rec_buf, |lba, out| {
            let off = lba as usize * 512;
            if off + 512 <= img.len() {
                out.copy_from_slice(&img[off..off + 512]);
                true
            } else {
                false
            }
        })?;
        Some((vol, rec_buf, [0u8; 4096]))
    }

    fn reader<'a>(img: &'a [u8]) -> impl FnMut(u32, &mut [u8; 512]) -> bool + 'a {
        move |lba, out| {
            let off = lba as usize * 512;
            if off + 512 <= img.len() {
                out.copy_from_slice(&img[off..off + 512]);
                true
            } else {
                false
            }
        }
    }

    fn path(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn mount_superfloppy_geometry_label_and_free_space() {
        let img = build_image(0);
        let (vol, mut rec_buf, _) = mount(&img).expect("mount");
        assert_eq!(vol.part_lba, 0);
        assert_eq!(vol.sec_per_clus, 1);
        assert_eq!(vol.cluster_count, TOTAL_CLUSTERS);
        assert_eq!(vol.mft_record_size, 1024);
        assert_eq!(vol.index_record_size, 4096);
        assert_eq!(vol.serial, SERIAL);
        let label: String = vol.label[..vol.label_len as usize]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('?'))
            .collect();
        assert_eq!(label, "NTFSTEST");
        // Used: boot 1 + $MFT 64 + bitmap 1 + BIG 8 + INDX 16 = 90.
        assert_eq!(free_clusters(&vol, &mut rec_buf,  reader(&img)), Some(310));
    }

    #[test]
    fn mount_via_mbr_partition() {
        let img = build_image(40);
        let (vol, _, _) = mount(&img).expect("mount via MBR");
        assert_eq!(vol.part_lba, 40);
        assert_eq!(vol.cluster_count, TOTAL_CLUSTERS);
    }

    #[test]
    fn resident_file_read_via_find_path() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        let e = find_path(&vol, &path("HELLO.TXT"), &mut rec_buf, &mut indx_buf,  reader(&img))
            .expect("HELLO.TXT found");
        assert_eq!(e.record, 16);
        assert!(!e.is_dir);
        assert_eq!(e.data_len, HELLO_PAYLOAD.len() as u64);
        let mut out = [0u8; 64];
        let n = read_entry_data(&vol, &e, 0, &mut out, &mut rec_buf,  reader(&img));
        assert_eq!(&out[..n], HELLO_PAYLOAD);
    }

    #[test]
    fn runlist_read_with_sparse_gap() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        let e = find_path(&vol, &path("BIG.BIN"), &mut rec_buf, &mut indx_buf,  reader(&img))
            .expect("BIG.BIN found");
        assert_eq!(e.record, 17);
        let mut out = vec![0u8; 5120];
        let n = read_entry_data(&vol, &e, 0, &mut out, &mut rec_buf,  reader(&img));
        assert_eq!(n, 5120);
        assert_eq!(out, big_payload());
        // A window straddling the sparse gap: pattern, zeros, pattern.
        let mut win = [0u8; 200];
        let n = read_entry_data(&vol, &e, 2000, &mut win, &mut rec_buf,  reader(&img));
        assert_eq!(n, 200);
        assert_eq!(&win[..], &big_payload()[2000..2200]);
        // A window starting inside the gap and ending in run B.
        let n = read_entry_data(&vol, &e, 3000, &mut win, &mut rec_buf,  reader(&img));
        assert_eq!(n, 200);
        assert_eq!(&win[..], &big_payload()[3000..3200]);
    }

    #[test]
    fn subdirectory_lookup_and_listing() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        let e = find_path(
            &vol,
            &path("games/doom.txt"),
            &mut rec_buf,
            &mut indx_buf,
            reader(&img),
        )
        .expect("DOOM.TXT found via GAMES");
        assert_eq!(e.record, 19);
        let mut out = [0u8; 64];
        let n = read_entry_data(&vol, &e, 0, &mut out, &mut rec_buf,  reader(&img));
        assert_eq!(&out[..n], DOOM_PAYLOAD);

        let mut names: Vec<String> = Vec::new();
        for_each_dir_entry(
            &vol,
            18,
            &mut rec_buf,
            &mut indx_buf,
            reader(&img),
            |e| {
                names.push(e.name[..e.name_len as usize].iter().map(|&c| c as u8 as char).collect());
                true
            },
        );
        assert_eq!(names, ["DOOM.TXT"]);
    }

    #[test]
    fn root_listing_walks_index_root_and_both_indx_blocks() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        let mut names: Vec<String> = Vec::new();
        for_each_dir_entry(
            &vol,
            ROOT_RECORD,
            &mut rec_buf,
            &mut indx_buf,
            reader(&img),
            |e| {
                names.push(e.name[..e.name_len as usize].iter().map(|&c| c as u8 as char).collect());
                true
            },
        );
        names.sort();
        // GONE.TXT appears: enumeration reports the index as the
        // filesystem maintains it, stale entries included; the next
        // test proves lookups refuse it.
        assert_eq!(
            names,
            ["BIG.BIN", "GAMES", "GONE.TXT", "HELLO.TXT", "LONGFI~1.TXT", "LongFileNameTest.txt"]
        );
    }

    #[test]
    fn long_name_lookup_prefers_win32_and_dos_alias_resolves() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        let e = find_path(&vol, &path(LONG_NAME), &mut rec_buf, &mut indx_buf,  reader(&img))
            .expect("long name found");
        assert_eq!(e.record, 20);
        let name: String = e.name[..e.name_len as usize]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('?'))
            .collect();
        assert_eq!(name, LONG_NAME);
        // The DOS alias finds the same record, and the returned entry
        // still carries the record's preferred (Win32) name.
        let e2 = find_path(&vol, &path(DOS_NAME), &mut rec_buf, &mut indx_buf,  reader(&img))
            .expect("DOS alias found");
        assert_eq!(e2.record, 20);
        assert_eq!(e2.name_len, e.name_len);
        let mut out = [0u8; 64];
        let n = read_entry_data(&vol, &e, 0, &mut out, &mut rec_buf,  reader(&img));
        assert_eq!(&out[..n], LONG_PAYLOAD);
    }

    #[test]
    fn deleted_record_is_invisible_to_lookup_and_read() {
        let img = build_image(0);
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount");
        // The root index still carries a stale GONE.TXT entry, but the
        // record's in-use flag is clear, so the lookup must refuse it.
        assert!(
            find_path(&vol, &path("GONE.TXT"), &mut rec_buf, &mut indx_buf,  reader(&img)).is_none()
        );
        let ghost = NtfsEntry {
            name: [0u16; 128],
            name_len: 0,
            name_truncated: false,
            record: 21,
            is_dir: false,
            data_len: 4,
            modified: 0,
        };
        let mut out = [0u8; 16];
        assert_eq!(
            read_entry_data(&vol, &ghost, 0, &mut out, &mut rec_buf,  reader(&img)),
            0
        );
    }

    #[test]
    fn corrupted_fixup_is_rejected() {
        // Tear the tail of HELLO.TXT's record (record 16, second
        // sector): the lookup path must refuse the record.
        let mut img = build_image(0);
        let tail = (MFT_LCN as usize + 16 * 2) * 512 + 1023;
        img[tail] ^= 0xFF;
        let (vol, mut rec_buf, mut indx_buf) = mount(&img).expect("mount still works");
        assert!(
            find_path(&vol, &path("HELLO.TXT"), &mut rec_buf, &mut indx_buf,  reader(&img))
                .is_none()
        );
        // Tear record 0 itself: the volume must not mount at all.
        let mut img = build_image(0);
        let tail0 = (MFT_LCN as usize) * 512 + 511;
        img[tail0] ^= 0xFF;
        assert!(mount(&img).is_none());
    }

    #[test]
    fn wrong_oem_is_rejected() {
        let mut img = build_image(0);
        img[3..11].copy_from_slice(b"EXFAT   ");
        assert!(mount(&img).is_none());
        let mut img = build_image(0);
        img[3..11].copy_from_slice(b"MSDOS5.0");
        assert!(mount(&img).is_none());
    }
}
