//! exFAT read-only parser — the filesystem core for the INT 2Fh redirector
//! work item (HANDOFF, owner-approved 2026-11-24).
//!
//! Pure `#![no_std]`, zero-heap, sector-callback reader in the same shape as
//! `fat_ro.rs`. Contains ZERO write logic by construction: exFAT support
//! starts read-only, and write support is a separate follow-on item.
//!
//! Format reference: Microsoft's published exFAT specification (2019).
//! Directory entries arrive in sets (file + stream extension + name
//! entries); every set carries a checksum, and a set whose checksum fails
//! is skipped rather than trusted — directory data is untrusted input,
//! exactly like the FAT tables in `fat_ro.rs`.
//!
//! v1 limits, stated plainly: 512-byte-sector volumes only (the sector
//! callback this crate standardised on; SD cards and USB sticks are
//! 512-byte media), MBR partitioning (a GPT protective partition is
//! skipped, not followed), and names longer than 128 UTF-16 code units
//! are reported truncated.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ExfatVolume {
    pub part_lba: u32,
    pub sec_per_clus: u32,
    pub fat_lba: u32,
    pub fat_secs: u32,
    pub heap_lba: u32,
    pub cluster_count: u32,
    pub root_cluster: u32,
    pub serial: u32,
    // Root-directory system entries, filled by `mount_volume`.
    pub bitmap_cluster: u32,
    pub bitmap_len: u64,
    pub upcase_cluster: u32,
    pub upcase_len: u64,
    pub upcase_checksum: u32,
    pub label: [u16; 11],
    pub label_len: u8,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ExfatEntry {
    pub name: [u16; 128],
    pub name_len: u8,
    pub name_truncated: bool,
    pub attr: u16,
    pub is_dir: bool,
    pub first_cluster: u32,
    pub data_len: u64,
    pub valid_len: u64,
    pub no_fat_chain: bool,
    /// Raw exFAT modified timestamp (see `decode_timestamp`).
    pub modified: u32,
}

pub const ATTR_DIRECTORY: u16 = 0x10;

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

/// One step of the 32-bit rotate-right-and-add checksum the spec uses
/// for the boot region and the up-case table. The entry-set checksum
/// and the name hash are 16-bit instead (see set_checksum) — the spec
/// gives the two families different widths, and folding a set with
/// this 32-bit step produces a value no real formatter agrees with,
/// which makes every set on a Windows-formatted volume fail
/// verification and silently vanish from listings.
fn cksum_step(sum: u32, byte: u8) -> u32 {
    let rotated = (sum >> 1) | ((sum & 1) << 31);
    rotated.wrapping_add(byte as u32)
}

/// Checksum of a directory entry set: every byte of every entry, except
/// bytes 2..4 of the first entry (the checksum field itself), folded
/// with a 16-bit rotate-right-and-add.
pub fn set_checksum(entries: &[u8]) -> u16 {
    let mut sum: u16 = 0;
    for (i, &byte) in entries.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        let rotated = (sum >> 1) | ((sum & 1) << 15);
        sum = rotated.wrapping_add(byte as u16);
    }
    sum
}

/// Fold one boot sector into a running boot checksum. Bytes 106, 107
/// (VolumeFlags) and 112 (PercentInUse) of sector 0 are excluded
/// because they change at runtime. Folding sector by sector keeps the
/// verifier's stack frame to a single sector buffer: the JLM calls it
/// on an 8 KiB private stack, where materializing the whole 11-sector
/// region at once (5,632 bytes) overran the stack and hung the machine.
fn boot_checksum_sector(sum: u32, sector_index: u32, sec: &[u8; 512]) -> u32 {
    let mut sum = sum;
    for (off, &byte) in sec.iter().enumerate() {
        if sector_index == 0 && (off == 106 || off == 107 || off == 112) {
            continue;
        }
        sum = cksum_step(sum, byte);
    }
    sum
}

/// Checksum of the 11 main boot sectors, passed as whole sectors; the
/// checksum sector (sector 11) stores the result repeated over the
/// whole sector.
pub fn boot_checksum(sectors: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for (s, chunk) in sectors.chunks_exact(512).enumerate() {
        let sec: &[u8; 512] = chunk.try_into().expect("whole sectors only");
        sum = boot_checksum_sector(sum, s as u32, sec);
    }
    sum
}

/// The spec's name hash: rotate-right-and-add over the up-cased name in
/// UTF-16LE, in a 16-bit accumulator.
pub fn name_hash_upcased(upcased_utf16le: &[u8]) -> u16 {
    let mut hash: u16 = 0;
    for &byte in upcased_utf16le {
        hash = ((hash >> 1) | ((hash & 1) << 15)).wrapping_add(byte as u16);
    }
    hash
}

/// Decode an exFAT timestamp into (year, month, day, hour, minute, second).
/// The layout is DOS-like but not identical to FAT's: year is years since
/// 1980 in the top 7 bits, seconds are stored divided by two.
pub fn decode_timestamp(v: u32) -> (u16, u8, u8, u8, u8, u8) {
    let sec = ((v & 0x1F) * 2) as u8;
    let min = ((v >> 5) & 0x3F) as u8;
    let hour = ((v >> 11) & 0x1F) as u8;
    let day = ((v >> 16) & 0x1F) as u8;
    let month = ((v >> 21) & 0x0F) as u8;
    let year = ((v >> 25) & 0x7F) as u16 + 1980;
    (year, month, day, hour, min, sec)
}

/// Parse an exFAT VBR (the partition's first sector) into volume geometry.
/// System entries (bitmap / up-case / label) are left zeroed; use
/// `mount_volume` for a fully populated volume.
pub fn parse_vbr(sec: &[u8; 512], part_lba: u32) -> Option<ExfatVolume> {
    // JumpBoot + the "EXFAT   " OEM name are the format's signature.
    if sec[0] != 0xEB || sec[1] != 0x76 || sec[2] != 0x90 {
        return None;
    }
    if &sec[3..11] != b"EXFAT   " {
        return None;
    }
    // Bytes 11..64 are MustBeZero in the spec; a nonzero byte there means
    // this is not really an exFAT boot sector.
    if sec[11..64].iter().any(|&b| b != 0) {
        return None;
    }
    // Only spec revision 1.00 exists; refuse anything else rather than
    // guessing at a future layout.
    if r16(sec, 104) != 0x0100 {
        return None;
    }
    // v1 limit: 512-byte sectors (see module docs).
    if sec[108] != 9 {
        return None;
    }
    let spc_shift = sec[109];
    if spc_shift > 16 {
        return None;
    }
    let sec_per_clus = 1u32 << spc_shift;
    let num_fats = sec[110] as u32;
    if num_fats == 0 || num_fats > 2 {
        return None;
    }
    let volume_len = r64(sec, 72);
    let fat_offset = r32(sec, 80);
    let fat_length = r32(sec, 84);
    let heap_offset = r32(sec, 88);
    let cluster_count = r32(sec, 92);
    let root_cluster = r32(sec, 96);
    if fat_offset == 0 || fat_length == 0 || heap_offset == 0 || cluster_count < 4 {
        return None;
    }
    // The FAT region and the cluster heap must sit inside the volume, in
    // that order, with checked arithmetic: every field here is untrusted,
    // and a wrapped sum would mount a geometry that reads foreign sectors.
    let fat_end = fat_offset.checked_add(fat_length.checked_mul(num_fats)?)?;
    if fat_end > heap_offset {
        return None;
    }
    let heap_secs = (cluster_count as u64).checked_mul(sec_per_clus as u64)?;
    let heap_end = (heap_offset as u64).checked_add(heap_secs)?;
    if heap_end > volume_len {
        return None;
    }
    if !valid_cluster_raw(cluster_count, root_cluster) {
        return None;
    }
    let fat_lba = part_lba.checked_add(fat_offset)?;
    let heap_lba = part_lba.checked_add(heap_offset)?;
    // Every LBA this volume can produce must fit the u32 sector callback.
    heap_lba.checked_add(heap_secs as u32)?;

    Some(ExfatVolume {
        part_lba,
        sec_per_clus,
        fat_lba,
        fat_secs: fat_length,
        heap_lba,
        cluster_count,
        root_cluster,
        serial: r32(sec, 100),
        bitmap_cluster: 0,
        bitmap_len: 0,
        upcase_cluster: 0,
        upcase_len: 0,
        upcase_checksum: 0,
        label: [0u16; 11],
        label_len: 0,
    })
}

fn valid_cluster_raw(cluster_count: u32, clus: u32) -> bool {
    clus >= 2 && clus - 2 < cluster_count
}

/// Whether `clus` names a data cluster that exists on this volume.
fn valid_cluster(vol: &ExfatVolume, clus: u32) -> bool {
    valid_cluster_raw(vol.cluster_count, clus)
}

/// Convert a cluster number (`>= 2`) to its first LBA on disk.
pub fn cluster_to_lba(vol: &ExfatVolume, clus: u32) -> Option<u32> {
    if !valid_cluster(vol, clus) {
        return None;
    }
    vol.heap_lba
        .checked_add((clus - 2).checked_mul(vol.sec_per_clus)?)
}

/// Look up the next cluster in the FAT, returning `None` at end-of-chain
/// or on any inconsistency. exFAT FAT entries are plain u32 cluster
/// numbers; values >= 0xFFFFFFF8 mark the end of a chain.
pub fn next_cluster<F>(vol: &ExfatVolume, clus: u32, mut read_sec: F) -> Option<u32>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if !valid_cluster(vol, clus) {
        return None;
    }
    let byte_off = (clus as u64).checked_mul(4)?;
    let fat_sec = vol.fat_lba.checked_add((byte_off / 512) as u32)?;
    if fat_sec >= vol.fat_lba + vol.fat_secs {
        return None;
    }
    let mut sec = [0u8; 512];
    if !read_sec(fat_sec, &mut sec) {
        return None;
    }
    let val = r32(&sec, (byte_off % 512) as usize);
    if val >= 0xFFFF_FFF8 || !valid_cluster(vol, val) {
        None
    } else {
        Some(val)
    }
}

/// The cluster `index` clusters into a chain that starts at `first`.
/// Contiguous (NoFatChain) chains are arithmetic; FAT chains are walked.
fn chain_cluster_at<F>(
    vol: &ExfatVolume,
    first: u32,
    no_fat_chain: bool,
    index: u32,
    mut read_sec: F,
) -> Option<u32>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if !valid_cluster(vol, first) {
        return None;
    }
    if no_fat_chain {
        let clus = first.checked_add(index)?;
        return if valid_cluster(vol, clus) {
            Some(clus)
        } else {
            None
        };
    }
    let mut clus = first;
    for _ in 0..index {
        clus = next_cluster(vol, clus, &mut read_sec)?;
    }
    Some(clus)
}

/// Read bytes from a cluster chain (`first`, optionally FAT-less) starting
/// at byte offset `start`, bounded by the chain's declared length
/// `total_len`. Returns the number of bytes actually read; a short read
/// means the chain ended or the media errored.
pub fn read_chain<F>(
    vol: &ExfatVolume,
    first: u32,
    no_fat_chain: bool,
    total_len: u64,
    start: u64,
    out: &mut [u8],
    mut read_sec: F,
) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if start >= total_len || out.is_empty() {
        return 0;
    }
    let want = core::cmp::min(out.len() as u64, total_len - start) as usize;
    let cluster_bytes = (vol.sec_per_clus as u64) * 512;
    let mut done = 0usize;
    let mut pos = start;
    let mut sec = [0u8; 512];
    // Walk the chain once, tracking the current cluster so sequential
    // progress never re-walks from the head.
    let mut index = (pos / cluster_bytes) as u32;
    let mut clus = match chain_cluster_at(vol, first, no_fat_chain, index, &mut read_sec) {
        Some(c) => c,
        None => return 0,
    };
    while done < want {
        let in_cluster = pos % cluster_bytes;
        if in_cluster == 0 && done > 0 {
            index += 1;
            match chain_cluster_at(vol, first, no_fat_chain, index, &mut read_sec) {
                Some(c) => clus = c,
                None => break,
            }
        }
        let Some(base) = cluster_to_lba(vol, clus) else {
            break;
        };
        let sec_in_cluster = (in_cluster / 512) as u32;
        if !read_sec(base + sec_in_cluster, &mut sec) {
            break;
        }
        let off = (in_cluster % 512) as usize;
        let take = core::cmp::min(512 - off, want - done);
        out[done..done + take].copy_from_slice(&sec[off..off + take]);
        done += take;
        pos += take as u64;
    }
    done
}

/// Read file data for a directory entry, starting at byte offset `start`.
pub fn read_entry_data<F>(
    vol: &ExfatVolume,
    entry: &ExfatEntry,
    start: u64,
    out: &mut [u8],
    read_sec: F,
) -> usize
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if entry.first_cluster == 0 {
        return 0;
    }
    read_chain(
        vol,
        entry.first_cluster,
        entry.no_fat_chain,
        entry.data_len,
        start,
        out,
        read_sec,
    )
}

fn ascii_upcase(ch: u16) -> u16 {
    if (0x61..=0x7A).contains(&ch) {
        ch - 0x20
    } else {
        ch
    }
}

/// Up-case one character. ASCII uses the mapping every valid up-case
/// table shares; anything above ASCII is resolved through the volume's
/// table, whose on-disk form compresses identity runs behind 0xFFFF
/// escape entries — the walk decodes runs until it passes `ch`.
pub fn upcase_char<F>(vol: &ExfatVolume, ch: u16, mut read_sec: F) -> u16
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if ch < 0x80 {
        return ascii_upcase(ch);
    }
    if vol.upcase_cluster == 0 || vol.upcase_len == 0 {
        return ch;
    }
    let mut codepoint: u32 = 0;
    let mut offset: u64 = 0;
    let mut word = [0u8; 2];
    while offset + 2 <= vol.upcase_len {
        if read_chain(vol, vol.upcase_cluster, false, vol.upcase_len, offset, &mut word, &mut read_sec) != 2 {
            return ch;
        }
        let v = u16::from_le_bytes(word);
        offset += 2;
        if v == 0xFFFF {
            if read_chain(vol, vol.upcase_cluster, false, vol.upcase_len, offset, &mut word, &mut read_sec) != 2 {
                return ch;
            }
            let count = u16::from_le_bytes(word) as u32;
            offset += 2;
            if (ch as u32) < codepoint + count {
                return ch; // identity run: maps to itself
            }
            codepoint += count;
        } else {
            if codepoint == ch as u32 {
                return v;
            }
            codepoint += 1;
        }
        if codepoint > ch as u32 {
            return ch;
        }
    }
    ch
}

/// The spec name hash of a name, up-cased through the volume's table.
pub fn name_hash<F>(vol: &ExfatVolume, name: &[u16], mut read_sec: F) -> u16
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut hash: u16 = 0;
    for &ch in name {
        let up = upcase_char(vol, ch, &mut read_sec);
        for &b in up.to_le_bytes().iter() {
            hash = ((hash >> 1) | ((hash & 1) << 15)).wrapping_add(b as u16);
        }
    }
    hash
}

/// Case-insensitive name equality through the volume's up-case rules.
fn name_eq<F>(vol: &ExfatVolume, a: &[u16], b: &[u16], mut read_sec: F) -> bool
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if upcase_char(vol, a[i], &mut read_sec) != upcase_char(vol, b[i], &mut read_sec) {
            return false;
        }
    }
    true
}

/// Walk one directory cluster by cluster, handing each raw 32-byte
/// entry to `cb`. Returning false from `cb` stops the walk; a 0x00
/// entry type (end of directory) always stops it. The low-level pass
/// `mount_volume` uses for the root's system entries; file sets are
/// assembled by `for_each_dir_entry`.
fn for_each_raw_entry<F, C>(vol: &ExfatVolume, dir_cluster: u32, mut read_sec: F, mut cb: C)
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
    C: FnMut(&[u8; 32]) -> bool,
{
    let mut sec = [0u8; 512];
    let mut clus = dir_cluster;
    let mut walked = 0u32;
    loop {
        if !valid_cluster(vol, clus) {
            return;
        }
        let Some(base) = cluster_to_lba(vol, clus) else {
            return;
        };
        for s in 0..vol.sec_per_clus {
            if !read_sec(base + s, &mut sec) {
                return;
            }
            for i in 0..16usize {
                let mut e = [0u8; 32];
                e.copy_from_slice(&sec[i * 32..(i + 1) * 32]);
                if e[0] == 0x00 {
                    return; // end of directory
                }
                if !cb(&e) {
                    return;
                }
            }
        }
        walked += 1;
        if walked > vol.cluster_count {
            return; // cyclic chain: stop rather than spin
        }
        match next_cluster(vol, clus, &mut read_sec) {
            Some(next) => clus = next,
            None => return,
        }
    }
}

/// Iterate the file entries of a directory (`dir_cluster`; pass
/// `vol.root_cluster` for the root). Entry sets are assembled whole and
/// their checksums verified before anything is reported: deleted sets,
/// stray secondaries, and checksum-failing sets are all skipped.
pub fn for_each_dir_entry<F, C>(vol: &ExfatVolume, dir_cluster: u32, mut read_sec: F, mut cb: C)
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
    C: FnMut(&ExfatEntry) -> bool,
{
    // Set assembly state. `remaining` counts the secondaries still owed
    // by the current primary; while it is nonzero we are inside a set,
    // whether the set is live (`cur` is Some) or being skipped (deleted,
    // unknown, or oversized — its secondaries must still be consumed to
    // keep the stream in sync).
    let mut set_bytes = [0u8; 32 * 21];
    let mut set_len = 0usize;
    let mut remaining = 0usize;
    let mut cur: Option<ExfatEntry> = None;
    let mut name_target = 0usize;
    let mut name_collected = 0usize;
    let mut stream_seen = false;

    for_each_raw_entry(vol, dir_cluster, &mut read_sec, |raw| {
        let ty = raw[0];
        if remaining == 0 {
            // Expecting a primary or a standalone system entry.
            if ty == 0x81 || ty == 0x82 || ty == 0x83 {
                return true; // system entry: single, skip
            }
            if ty & 0x40 != 0 {
                return true; // stray secondary with no primary: ignore
            }
            // Primary entry (bit 6 clear). Its SecondaryCount says how
            // many entries follow, live or deleted alike.
            let count = raw[1] as usize;
            if count == 0 {
                return true; // a file set always has at least a stream entry
            }
            remaining = count;
            set_len = 0;
            name_target = 0;
            name_collected = 0;
            stream_seen = false;
            if ty == 0x85 && count <= 20 {
                set_bytes[0..32].copy_from_slice(raw);
                set_len = 32;
                cur = Some(ExfatEntry {
                    name: [0u16; 128],
                    name_len: 0,
                    name_truncated: false,
                    attr: r16(raw, 4),
                    is_dir: r16(raw, 4) & ATTR_DIRECTORY != 0,
                    first_cluster: 0,
                    data_len: 0,
                    valid_len: 0,
                    no_fat_chain: false,
                    modified: r32(raw, 12),
                });
            } else {
                // Deleted, unknown, or implausibly large set: consume its
                // secondaries without accumulating or emitting.
                cur = None;
            }
            return true;
        }
        // Inside a set: consume one secondary.
        remaining -= 1;
        if cur.is_some() {
            if set_len + 32 <= set_bytes.len() {
                set_bytes[set_len..set_len + 32].copy_from_slice(raw);
                set_len += 32;
            }
            let mut drop_set = false;
            let e = cur.as_mut().unwrap();
            if !stream_seen {
                if ty == 0xC0 {
                    stream_seen = true;
                    e.no_fat_chain = raw[1] & 0x02 != 0;
                    name_target = raw[3] as usize;
                    e.valid_len = r64(raw, 8);
                    e.first_cluster = r32(raw, 20);
                    e.data_len = r64(raw, 24);
                } else {
                    // First secondary is not the stream extension: the
                    // set is malformed, drop it but keep consuming.
                    drop_set = true;
                }
            } else if ty == 0xC1 {
                for i in 0..15usize {
                    if name_collected >= name_target {
                        break;
                    }
                    let ch = r16(raw, 2 + i * 2);
                    if name_collected < e.name.len() {
                        e.name[name_collected] = ch;
                    }
                    name_collected += 1;
                }
            }
            if drop_set {
                cur = None;
            }
        }
        if remaining == 0 {
            if let Some(mut e) = cur.take() {
                let stored = r16(&set_bytes, 2);
                let complete = stream_seen && name_collected == name_target;
                if complete && set_checksum(&set_bytes[..set_len]) == stored {
                    e.name_truncated = name_target > e.name.len();
                    e.name_len = core::cmp::min(name_target, e.name.len()) as u8;
                    if !cb(&e) {
                        return false;
                    }
                }
            }
            set_len = 0;
        }
        true
    });
}

/// Look up a path (components separated by `\` or `/`, as UTF-16 code
/// units) starting from the root directory.
pub fn find_path<F>(vol: &ExfatVolume, path: &[u16], mut read_sec: F) -> Option<ExfatEntry>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut cur_dir = vol.root_cluster;
    let mut rest = path;
    while !rest.is_empty() && (rest[0] == b'\\' as u16 || rest[0] == b'/' as u16) {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return None;
    }
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
        // Up-case the query once, before the walk: the walk's callback
        // cannot borrow the reader (the walk owns it), so comparisons
        // inside the walk use the ASCII fast path, and any entry whose
        // name reaches above ASCII is collected as a candidate and
        // verified with the full table-driven comparison afterwards.
        let mut upq = [0u16; 128];
        for i in 0..comp.len() {
            upq[i] = upcase_char(vol, comp[i], &mut read_sec);
        }
        let upq = &upq[..comp.len()];
        let mut ascii_hit: Option<ExfatEntry> = None;
        let mut candidates: [Option<ExfatEntry>; 4] = [None; 4];
        let mut ncand = 0usize;
        for_each_dir_entry(vol, cur_dir, &mut read_sec, |e| {
            if e.name_truncated || e.name_len as usize != comp.len() {
                return true;
            }
            let name = &e.name[..comp.len()];
            let mut plausible = true;
            let mut all_ascii = true;
            for i in 0..comp.len() {
                if name[i] < 0x80 && upq[i] < 0x80 {
                    if ascii_upcase(name[i]) != upq[i] {
                        plausible = false;
                        break;
                    }
                } else {
                    all_ascii = false;
                }
            }
            if !plausible {
                return true;
            }
            if all_ascii {
                ascii_hit = Some(*e);
                return false; // ASCII comparison is exact: done
            }
            if ncand < candidates.len() {
                candidates[ncand] = Some(*e);
                ncand += 1;
            }
            true
        });
        let mut found = ascii_hit;
        if found.is_none() {
            for cand in candidates.iter().flatten() {
                if name_eq(vol, &cand.name[..cand.name_len as usize], comp, &mut read_sec) {
                    found = Some(*cand);
                    break;
                }
            }
        }
        let entry = found?;
        if sep >= rest.len() {
            return Some(entry);
        }
        if !entry.is_dir {
            return None;
        }
        cur_dir = entry.first_cluster;
        rest = &rest[sep + 1..];
        while !rest.is_empty() && (rest[0] == b'\\' as u16 || rest[0] == b'/' as u16) {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return Some(entry);
        }
    }
}

/// Count free clusters from the allocation bitmap.
pub fn free_clusters<F>(vol: &ExfatVolume, mut read_sec: F) -> Option<u32>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if vol.bitmap_cluster == 0 || vol.bitmap_len == 0 {
        return None;
    }
    let mut free = 0u32;
    let mut buf = [0u8; 512];
    let mut offset: u64 = 0;
    while offset < vol.bitmap_len {
        let n = read_chain(
            vol,
            vol.bitmap_cluster,
            false,
            vol.bitmap_len,
            offset,
            &mut buf,
            &mut read_sec,
        );
        if n == 0 {
            return None;
        }
        for i in 0..n {
            let base = (offset as usize + i) * 8;
            for bit in 0..8usize {
                if base + bit >= vol.cluster_count as usize {
                    break;
                }
                if buf[i] & (1 << bit) == 0 {
                    free += 1;
                }
            }
        }
        offset += n as u64;
    }
    Some(free)
}

/// Verify the main boot region's checksum sector against sectors 0..11.
fn boot_checksum_ok<F>(part_lba: u32, mut read_sec: F) -> bool
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    // Streamed one sector at a time (see boot_checksum_sector for why
    // the whole region must never sit on this stack).
    let mut expect: u32 = 0;
    let mut sec = [0u8; 512];
    for s in 0..11u32 {
        if !read_sec(part_lba + s, &mut sec) {
            return false;
        }
        expect = boot_checksum_sector(expect, s, &sec);
    }
    if !read_sec(part_lba + 11, &mut sec) {
        return false;
    }
    for i in 0..128 {
        if r32(&sec, i * 4) != expect {
            return false;
        }
    }
    true
}

/// Verify the up-case table's stored checksum against its contents.
fn upcase_checksum_ok<F>(vol: &ExfatVolume, mut read_sec: F) -> bool
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut sum: u32 = 0;
    let mut buf = [0u8; 512];
    let mut offset: u64 = 0;
    while offset < vol.upcase_len {
        let n = read_chain(
            vol,
            vol.upcase_cluster,
            false,
            vol.upcase_len,
            offset,
            &mut buf,
            &mut read_sec,
        );
        if n == 0 {
            return false;
        }
        for i in 0..n {
            sum = cksum_step(sum, buf[i]);
        }
        offset += n as u64;
    }
    sum == vol.upcase_checksum
}

/// Inspect sector 0: if it is an exFAT VBR (superfloppy), mount directly;
/// otherwise scan MBR partitions (type 0x07 carries exFAT as well as
/// NTFS/HPFS — the VBR signature decides) for the first exFAT volume.
/// The boot checksum is verified, and the root's system entries
/// (allocation bitmap, up-case table, volume label) are populated.
pub fn mount_volume<F>(mut read_sec: F) -> Option<ExfatVolume>
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
        let Some(mut vol) = parse_vbr(&vbr, part_lba) else {
            continue;
        };
        if !boot_checksum_ok(part_lba, &mut read_sec) {
            continue;
        }
        // Populate system entries from the root directory's raw stream.
        // Locals first: the walk borrows them while `vol` is only read.
        let mut bitmap_cluster = 0u32;
        let mut bitmap_len = 0u64;
        let mut upcase_cluster = 0u32;
        let mut upcase_len = 0u64;
        let mut upcase_checksum = 0u32;
        let mut label = [0u16; 11];
        let mut label_len = 0u8;
        for_each_raw_entry(&vol, vol.root_cluster, &mut read_sec, |raw| {
            match raw[0] {
                0x81 => {
                    bitmap_cluster = r32(raw, 20);
                    bitmap_len = r64(raw, 24);
                }
                0x82 => {
                    upcase_checksum = r32(raw, 4);
                    upcase_cluster = r32(raw, 20);
                    upcase_len = r64(raw, 24);
                }
                0x83 => {
                    label_len = core::cmp::min(raw[1], 11);
                    for i in 0..11usize {
                        label[i] = r16(raw, 2 + i * 2);
                    }
                }
                _ => {}
            }
            true
        });
        vol.bitmap_cluster = bitmap_cluster;
        vol.bitmap_len = bitmap_len;
        vol.upcase_cluster = upcase_cluster;
        vol.upcase_len = upcase_len;
        vol.upcase_checksum = upcase_checksum;
        vol.label = label;
        vol.label_len = label_len;
        if vol.bitmap_cluster == 0 || vol.upcase_cluster == 0 {
            continue;
        }
        if !upcase_checksum_ok(&vol, &mut read_sec) {
            continue;
        }
        return Some(vol);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic exFAT image, built byte by byte so every checksum in it
    // is genuinely computed: 512-byte sectors and clusters, FAT at
    // sector 24, cluster heap at sector 28, clusters 2..=12.
    //
    // Cluster map: 2 bitmap, 3 up-case table, 4 root directory,
    // 5 HELLO.TXT data (2nd), 6 GAMES directory (1st), 7 README data,
    // 8 HELLO.TXT data (1st), 9-10 DOOM.EXE data (contiguous),
    // 11 GAMES directory (2nd), 12 free.
    const FAT_OFF: usize = 24;
    const HEAP_OFF: usize = 28;
    const CLUSTERS: u32 = 11;

    fn coff(clus: u32) -> usize {
        (HEAP_OFF + (clus as usize - 2)) * 512
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

    fn ascii_u16(s: &str) -> Vec<u16> {
        s.chars().map(|c| c as u16).collect()
    }

    /// Write one file entry set (primary + stream + name) at byte offset
    /// `at` in the image; returns the offset just past the set.
    fn write_set(
        d: &mut [u8],
        at: usize,
        name: &str,
        attr: u16,
        stream_flags: u8,
        first_cluster: u32,
        data_len: u64,
        modified: u32,
    ) -> usize {
        let name16 = ascii_u16(name);
        let secondaries = 1 + (name16.len() + 14) / 15;
        // Primary
        d[at] = 0x85;
        d[at + 1] = secondaries as u8;
        w16(d, at + 4, attr);
        w32(d, at + 8, modified); // create
        w32(d, at + 12, modified); // modified
        w32(d, at + 16, modified); // accessed
        // Stream extension
        let s = at + 32;
        d[s] = 0xC0;
        d[s + 1] = stream_flags;
        d[s + 3] = name16.len() as u8;
        let mut up = Vec::new();
        for &ch in &name16 {
            up.extend_from_slice(&ascii_upcase(ch).to_le_bytes());
        }
        w16(d, s + 4, name_hash_upcased(&up));
        w64(d, s + 8, data_len); // valid data length
        w32(d, s + 20, first_cluster);
        w64(d, s + 24, data_len);
        // Name entries
        for (i, chunk) in name16.chunks(15).enumerate() {
            let n = at + 64 + i * 32;
            d[n] = 0xC1;
            for (j, &ch) in chunk.iter().enumerate() {
                w16(d, n + 2 + j * 2, ch);
            }
        }
        let total = 32 * (1 + secondaries);
        let sum = set_checksum(&d[at..at + total]);
        w16(d, at + 2, sum);
        at + total
    }

    fn write_deleted_set(d: &mut [u8], at: usize) -> usize {
        d[at] = 0x05; // deleted primary
        d[at + 1] = 2; // two secondaries follow
        d[at + 32] = 0x40; // deleted stream entry
        d[at + 64] = 0x41; // deleted name entry
        at + 96
    }

    fn build_image(base: u32) -> Vec<u8> {
        let b = (base as usize) * 512;
        let mut d = vec![0u8; b + 40 * 512];
        // ---- VBR (sector 0 of the volume)
        let v = b;
        d[v] = 0xEB;
        d[v + 1] = 0x76;
        d[v + 2] = 0x90;
        d[v + 3..v + 11].copy_from_slice(b"EXFAT   ");
        w64(&mut d, v + 72, 39); // volume length in sectors
        w32(&mut d, v + 80, FAT_OFF as u32);
        w32(&mut d, v + 84, 4); // FAT length
        w32(&mut d, v + 88, HEAP_OFF as u32);
        w32(&mut d, v + 92, CLUSTERS);
        w32(&mut d, v + 96, 4); // root directory cluster
        w32(&mut d, v + 100, 0x1234_5678); // serial
        w16(&mut d, v + 104, 0x0100); // revision 1.00
        d[v + 108] = 9; // 512-byte sectors
        d[v + 109] = 0; // 1 sector per cluster
        d[v + 110] = 1; // one FAT
        d[v + 510] = 0x55;
        d[v + 511] = 0xAA;
        // Boot checksum sector (sector 11)
        let sum = boot_checksum(&d[v..v + 11 * 512]);
        for i in 0..128 {
            w32(&mut d, v + 11 * 512 + i * 4, sum);
        }
        // ---- FAT
        let f = b + FAT_OFF * 512;
        let fat_set = |d: &mut [u8], clus: u32, val: u32| {
            w32(d, f + (clus as usize) * 4, val);
        };
        fat_set(&mut d, 0, 0xFFFF_FFF8);
        fat_set(&mut d, 1, 0xFFFF_FFFF);
        fat_set(&mut d, 2, 0xFFFF_FFFF);
        fat_set(&mut d, 3, 0xFFFF_FFFF);
        fat_set(&mut d, 4, 0xFFFF_FFFF);
        fat_set(&mut d, 5, 0xFFFF_FFFF);
        fat_set(&mut d, 6, 11); // GAMES continues at 11
        fat_set(&mut d, 7, 0xFFFF_FFFF);
        fat_set(&mut d, 8, 5); // HELLO.TXT continues at 5
        fat_set(&mut d, 11, 0xFFFF_FFFF);
        // Clusters 9 and 10 (DOOM.EXE) deliberately keep FAT entries of 0:
        // the file is NoFatChain, so the parser must not consult them.
        // ---- Allocation bitmap (cluster 2): clusters 2..=11 used, 12 free
        let bm = b + coff(2);
        d[bm] = 0xFF;
        d[bm + 1] = 0x03;
        // ---- Up-case table (cluster 3), compressed identity-run form:
        // cp 0..0x60 identity, a-z -> A-Z, cp 0x7B..0xE8 identity, é -> É.
        let uc = b + coff(3);
        let mut words: Vec<u16> = vec![0xFFFF, 0x0061];
        for i in 0..26u16 {
            words.push(0x0041 + i);
        }
        words.push(0xFFFF);
        words.push(0x006E);
        words.push(0x00C9);
        for (i, &w) in words.iter().enumerate() {
            w16(&mut d, uc + i * 2, w);
        }
        let upcase_len = (words.len() * 2) as u64;
        let mut table_sum: u32 = 0;
        for i in 0..upcase_len as usize {
            table_sum = cksum_step(table_sum, d[uc + i]);
        }
        // ---- Root directory (cluster 4)
        let mut at = b + coff(4);
        // Allocation bitmap entry
        d[at] = 0x81;
        w32(&mut d, at + 20, 2);
        w64(&mut d, at + 24, 2);
        at += 32;
        // Up-case table entry
        d[at] = 0x82;
        w32(&mut d, at + 4, table_sum);
        w32(&mut d, at + 20, 3);
        w64(&mut d, at + 24, upcase_len);
        at += 32;
        // Volume label entry: "FERRIC"
        d[at] = 0x83;
        d[at + 1] = 6;
        for (i, &ch) in ascii_u16("FERRIC").iter().enumerate() {
            w16(&mut d, at + 2 + i * 2, ch);
        }
        at += 32;
        // 2026-11-24 03:00:00 in exFAT timestamp layout
        let ts = (46u32 << 25) | (11 << 21) | (24 << 16) | (3 << 11);
        at = write_set(&mut d, at, "HELLO.TXT", 0x20, 0x01, 8, 600, ts);
        at = write_set(&mut d, at, "GAMES", 0x10, 0x01, 6, 1024, ts);
        let _ = write_deleted_set(&mut d, at);
        // ---- GAMES directory (clusters 6 then 11)
        let mut g = b + coff(6);
        g = write_set(&mut d, g, "DOOM.EXE", 0x20, 0x03, 9, 700, ts);
        for _ in 0..4 {
            g = write_deleted_set(&mut d, g);
        }
        d[g] = 0x05; // one deleted primary, no secondaries, fills the cluster
        let mut g2 = b + coff(11);
        g2 = write_set(&mut d, g2, "README", 0x20, 0x01, 7, 10, ts);
        let _ = g2;
        // ---- File data
        for i in 0..512 {
            d[b + coff(8) + i] = b'A';
            d[b + coff(9) + i] = b'C';
        }
        for i in 0..88 {
            d[b + coff(5) + i] = b'B';
        }
        for i in 0..188 {
            d[b + coff(10) + i] = b'D';
        }
        for i in 0..10 {
            d[b + coff(7) + i] = b'R';
        }
        // ---- Optional MBR wrapper
        if base > 0 {
            d[510] = 0x55;
            d[511] = 0xAA;
            let p = 446;
            d[p] = 0x80; // bootable
            d[p + 4] = 0x07; // exFAT/NTFS partition type
            w32(&mut d, p + 8, base);
            w32(&mut d, p + 12, 39);
        }
        d
    }

    fn reader(disk: Vec<u8>) -> impl Fn(u32, &mut [u8; 512]) -> bool {
        move |lba: u32, out: &mut [u8; 512]| {
            let off = (lba as usize) * 512;
            if off + 512 <= disk.len() {
                out.copy_from_slice(&disk[off..off + 512]);
                true
            } else {
                false
            }
        }
    }

    fn path(s: &str) -> Vec<u16> {
        ascii_u16(s)
    }

    #[test]
    fn mount_superfloppy_populates_system_entries() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk)).expect("mount");
        assert_eq!(vol.root_cluster, 4);
        assert_eq!(vol.serial, 0x1234_5678);
        assert_eq!(vol.bitmap_cluster, 2);
        assert_eq!(vol.upcase_cluster, 3);
        assert_eq!(vol.label_len, 6);
        assert_eq!(&vol.label[..6], &ascii_u16("FERRIC")[..]);
        assert_eq!(free_clusters(&vol, reader(build_image(0))), Some(1));
    }

    #[test]
    fn mount_via_mbr_partition() {
        let disk = build_image(40);
        let vol = mount_volume(reader(disk)).expect("mount via MBR");
        assert_eq!(vol.part_lba, 40);
        let ent = find_path(&vol, &path("HELLO.TXT"), reader(build_image(40))).expect("find");
        assert_eq!(ent.data_len, 600);
    }

    #[test]
    fn find_and_read_across_a_fat_chain() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk.clone())).unwrap();
        // Lowercase query must match the stored uppercase name.
        let ent = find_path(&vol, &path("hello.txt"), reader(disk.clone())).expect("find");
        assert_eq!(ent.first_cluster, 8);
        assert_eq!(ent.data_len, 600);
        assert!(!ent.is_dir);
        assert_eq!(decode_timestamp(ent.modified), (2026, 11, 24, 3, 0, 0));
        // Read straddling the chain hop from cluster 8 to cluster 5.
        let mut buf = [0u8; 100];
        let n = read_entry_data(&vol, &ent, 500, &mut buf, reader(disk));
        assert_eq!(n, 100);
        assert!(buf[..12].iter().all(|&c| c == b'A'));
        assert!(buf[12..].iter().all(|&c| c == b'B'));
    }

    #[test]
    fn contiguous_file_ignores_the_fat() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk.clone())).unwrap();
        let ent = find_path(&vol, &path("GAMES\\DOOM.EXE"), reader(disk.clone())).expect("find");
        assert!(ent.no_fat_chain);
        assert_eq!(ent.data_len, 700);
        let mut buf = [0u8; 700];
        let n = read_entry_data(&vol, &ent, 0, &mut buf, reader(disk));
        assert_eq!(n, 700);
        assert!(buf[..512].iter().all(|&c| c == b'C'));
        assert!(buf[512..].iter().all(|&c| c == b'D'));
    }

    #[test]
    fn directory_walk_crosses_clusters_and_skips_deleted_sets() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk.clone())).unwrap();
        let mut root_names = 0;
        for_each_dir_entry(&vol, vol.root_cluster, reader(disk.clone()), |_| {
            root_names += 1;
            true
        });
        assert_eq!(root_names, 2); // HELLO.TXT + GAMES only
        let mut games_names: Vec<Vec<u16>> = Vec::new();
        for_each_dir_entry(&vol, 6, reader(disk.clone()), |e| {
            games_names.push(e.name[..e.name_len as usize].to_vec());
            true
        });
        assert_eq!(games_names.len(), 2);
        assert_eq!(games_names[0], ascii_u16("DOOM.EXE"));
        assert_eq!(games_names[1], ascii_u16("README"));
        let ent = find_path(&vol, &path("games/readme"), reader(disk)).expect("find README");
        assert_eq!(ent.first_cluster, 7);
    }

    #[test]
    fn upcase_table_drives_non_ascii_names() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk.clone())).unwrap();
        assert_eq!(upcase_char(&vol, 0xE9, reader(disk.clone())), 0xC9); // é -> É
        assert_eq!(upcase_char(&vol, 0x100, reader(disk.clone())), 0x100); // identity
        let h = name_hash(&vol, &[0xE9], reader(disk.clone()));
        assert_eq!(h, name_hash_upcased(&[0xC9, 0x00]));
    }

    #[test]
    fn cluster_bounds_are_enforced() {
        let disk = build_image(0);
        let vol = mount_volume(reader(disk)).unwrap();
        assert_eq!(cluster_to_lba(&vol, 1), None);
        assert_eq!(cluster_to_lba(&vol, 12), Some(vol.heap_lba + 10));
        assert_eq!(cluster_to_lba(&vol, 13), None);
        assert_eq!(next_cluster(&vol, 8, reader(build_image(0))), Some(5));
        assert_eq!(next_cluster(&vol, 5, reader(build_image(0))), None);
    }

    #[test]
    fn rejects_non_exfat_and_corrupt_images() {
        // A FAT-style VBR is not exFAT.
        let mut fat_sec = [0u8; 512];
        fat_sec[0] = 0xEB;
        fat_sec[1] = 0x3C;
        fat_sec[2] = 0x90;
        assert_eq!(parse_vbr(&fat_sec, 0), None);
        // A nonzero MustBeZero byte disqualifies the VBR.
        let mut disk = build_image(0);
        disk[20] = 1;
        assert_eq!(mount_volume(reader(disk)), None);
        // A corrupted boot checksum sector fails the mount.
        let mut disk = build_image(0);
        disk[11 * 512] ^= 0xFF;
        assert_eq!(mount_volume(reader(disk)), None);
        // A corrupted set checksum hides the file.
        let mut disk = build_image(0);
        let hello_name_byte = coff(4) + 3 * 32 + 64 + 2; // first name char of HELLO.TXT
        disk[hello_name_byte] ^= 0x01;
        let vol = mount_volume(reader(disk.clone())).unwrap();
        assert!(find_path(&vol, &path("HELLO.TXT"), reader(disk)).is_none());
    }
}
