//! Stage D: the exFAT writer core — the companion to `exfat_ro` that
//! adds allocation, file creation, data writes, and deletion on top of
//! the same sector-callback shape. All writes to the medium flow out of
//! this module through the caller's sector-writer callback (on target:
//! the Gate 3 USB MSC WRITE(10) plumbing in `ohci`); nothing here knows
//! about USB, DOS, or the redirector.
//!
//! Crash-ordering invariant, applied by every mutating operation in
//! this module: data clusters are written first, then the allocation
//! bitmap, then the FAT, and the directory entry set is written LAST.
//! An interruption anywhere in that sequence can orphan clusters
//! (allocated but unreachable — a space leak a checker can reclaim),
//! but it can never leave a directory entry pointing at unwritten data
//! or at clusters the bitmap still calls free. Deletion runs the
//! mirror image: the entry set is invalidated first and the clusters
//! are freed afterwards, for the same reason in reverse.
//!
//! The parser in `exfat_ro` remains the authority for reading: it
//! shares no code with this module beyond the checksum helpers, which
//! is what makes "write with this module, re-read with the parser" a
//! real verification in the tests below.

use crate::exfat_ro::{self, ExfatEntry, ExfatVolume};

const EOC: u32 = 0xFFFF_FFFF;

/// Bytes in one cluster on this volume.
fn cluster_bytes(vol: &ExfatVolume) -> u64 {
    vol.sec_per_clus as u64 * 512
}

fn valid_cluster(vol: &ExfatVolume, clus: u32) -> bool {
    clus >= 2 && clus - 2 < vol.cluster_count
}

// ---------------------------------------------------------------------------
// Chain addressing. Directories and the allocation bitmap are ordinary
// FAT chains; file data may be a contiguous run instead. Everything
// below reduces a (chain, byte offset) pair to a device LBA.
// ---------------------------------------------------------------------------

/// The cluster `index` clusters into a chain starting at `first`,
/// mirroring the parser's private walker from the public pieces.
fn chain_cluster_at<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    first: u32,
    no_fat_chain: bool,
    index: u32,
    mut rd: R,
) -> Option<u32> {
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
        clus = exfat_ro::next_cluster(vol, clus, &mut rd)?;
    }
    Some(clus)
}

/// LBA of the sector holding byte `byte_off` of a chain's stream.
fn chain_sector_lba<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    first: u32,
    no_fat_chain: bool,
    byte_off: u64,
    rd: R,
) -> Option<u32> {
    let sec_index = byte_off / 512;
    let clus_index = (sec_index / vol.sec_per_clus as u64) as u32;
    let clus = chain_cluster_at(vol, first, no_fat_chain, clus_index, rd)?;
    let base = exfat_ro::cluster_to_lba(vol, clus)?;
    Some(base + (sec_index % vol.sec_per_clus as u64) as u32)
}

/// Read-modify-write one device sector through the callbacks.
fn rmw_sector<R, W>(lba: u32, mut rd: R, mut wr: W, f: impl FnOnce(&mut [u8; 512])) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    let mut sec = [0u8; 512];
    if !rd(lba, &mut sec) {
        return false;
    }
    f(&mut sec);
    wr(lba, &sec)
}

// ---------------------------------------------------------------------------
// FAT access. Entries are plain u32 cluster numbers; a second FAT, when
// the volume carries one, is a mirror the spec expects to be kept in
// step, so every update lands in both.
// ---------------------------------------------------------------------------

fn num_fats<R: FnMut(u32, &mut [u8; 512]) -> bool>(vol: &ExfatVolume, mut rd: R) -> u32 {
    let mut sec = [0u8; 512];
    if rd(vol.part_lba, &mut sec) {
        let n = sec[110] as u32;
        if n == 1 || n == 2 {
            return n;
        }
    }
    1
}

fn get_fat<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    clus: u32,
    mut rd: R,
) -> Option<u32> {
    if !valid_cluster(vol, clus) {
        return None;
    }
    let byte_off = clus as u64 * 4;
    let lba = vol.fat_lba.checked_add((byte_off / 512) as u32)?;
    if lba >= vol.fat_lba + vol.fat_secs {
        return None;
    }
    let mut sec = [0u8; 512];
    if !rd(lba, &mut sec) {
        return None;
    }
    let o = (byte_off % 512) as usize;
    Some(u32::from_le_bytes([sec[o], sec[o + 1], sec[o + 2], sec[o + 3]]))
}

fn put_fat<R, W>(vol: &ExfatVolume, clus: u32, val: u32, mut rd: R, mut wr: W) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if !valid_cluster(vol, clus) {
        return false;
    }
    let byte_off = clus as u64 * 4;
    let sec_in_fat = (byte_off / 512) as u32;
    if sec_in_fat >= vol.fat_secs {
        return false;
    }
    let o = (byte_off % 512) as usize;
    let fats = num_fats(vol, &mut rd);
    for i in 0..fats {
        let lba = vol.fat_lba + i * vol.fat_secs + sec_in_fat;
        let ok = rmw_sector(lba, &mut rd, &mut wr, |sec| {
            sec[o..o + 4].copy_from_slice(&val.to_le_bytes());
        });
        if !ok {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Allocation bitmap. Bit (clus - 2) of the bitmap stream says whether
// the cluster is in use; the bitmap itself is a FAT chain rooted at
// the volume's bitmap cluster.
// ---------------------------------------------------------------------------

fn bitmap_lba<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    clus: u32,
    rd: R,
) -> Option<(u32, usize)> {
    let bit = (clus - 2) as u64;
    let byte_off = bit / 8;
    if byte_off >= vol.bitmap_len {
        return None;
    }
    let lba = chain_sector_lba(vol, vol.bitmap_cluster, false, byte_off, rd)?;
    Some((lba, (bit % 8) as usize))
}

fn test_bitmap<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    clus: u32,
    mut rd: R,
) -> Option<bool> {
    if !valid_cluster(vol, clus) {
        return None;
    }
    let (lba, bit) = bitmap_lba(vol, clus, &mut rd)?;
    let mut sec = [0u8; 512];
    if !rd(lba, &mut sec) {
        return None;
    }
    let byte = ((clus - 2) as u64 / 8 % 512) as usize;
    Some(sec[byte] & (1 << bit) != 0)
}

fn mark_bitmap<R, W>(vol: &ExfatVolume, clus: u32, used: bool, mut rd: R, mut wr: W) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if !valid_cluster(vol, clus) {
        return false;
    }
    let Some((lba, bit)) = bitmap_lba(vol, clus, &mut rd) else {
        return false;
    };
    let byte = ((clus - 2) as u64 / 8 % 512) as usize;
    rmw_sector(lba, &mut rd, &mut wr, |sec| {
        if used {
            sec[byte] |= 1 << bit;
        } else {
            sec[byte] &= !(1 << bit);
        }
    })
}

/// Scan the bitmap for the first run of `count` consecutive free
/// clusters. Bits past the end of the volume count as used: a run must
/// never spill into clusters that do not exist.
fn find_free_run<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    count: u32,
    mut rd: R,
) -> Option<u32> {
    if count == 0 || count > vol.cluster_count {
        return None;
    }
    let mut run_start = 0u32;
    let mut run_len = 0u32;
    let mut sec = [0u8; 512];
    let bitmap_secs = vol.bitmap_len.div_ceil(512);
    for s in 0..bitmap_secs {
        let lba = chain_sector_lba(vol, vol.bitmap_cluster, false, s * 512, &mut rd)?;
        if !rd(lba, &mut sec) {
            return None;
        }
        for i in 0..512usize {
            let base = (s as usize * 512 + i) * 8;
            if base as u64 >= vol.bitmap_len * 8 {
                break;
            }
            for bit in 0..8usize {
                let idx = base + bit;
                if idx >= vol.cluster_count as usize {
                    return None; // past the volume: no run completes here
                }
                if sec[i] & (1 << bit) == 0 {
                    if run_len == 0 {
                        run_start = idx as u32 + 2;
                    }
                    run_len += 1;
                    if run_len >= count {
                        return Some(run_start);
                    }
                } else {
                    run_len = 0;
                }
            }
        }
    }
    None
}

/// Collect up to `out.len()` free clusters, first-fit, anywhere in the
/// volume. The batch cap keeps the caller's stack bounded; allocation
/// rounds repeat until a request is satisfied.
fn collect_free<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    out: &mut [u32],
    mut rd: R,
) -> usize {
    let mut n = 0usize;
    let mut sec = [0u8; 512];
    let bitmap_secs = vol.bitmap_len.div_ceil(512);
    for s in 0..bitmap_secs {
        if n >= out.len() {
            break;
        }
        let Some(lba) = chain_sector_lba(vol, vol.bitmap_cluster, false, s * 512, &mut rd) else {
            break;
        };
        if !rd(lba, &mut sec) {
            break;
        }
        for i in 0..512usize {
            if n >= out.len() {
                break;
            }
            let base = (s as usize * 512 + i) * 8;
            for bit in 0..8usize {
                let idx = base + bit;
                if idx >= vol.cluster_count as usize {
                    return n;
                }
                if sec[i] & (1 << bit) == 0 {
                    out[n] = idx as u32 + 2;
                    n += 1;
                    if n >= out.len() {
                        break;
                    }
                }
            }
        }
    }
    n
}

/// True when every cluster in `[start, start + count)` is free.
fn range_free<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    start: u32,
    count: u32,
    mut rd: R,
) -> bool {
    if count == 0 {
        return true;
    }
    if start < 2 || start - 2 + count > vol.cluster_count {
        return false;
    }
    for i in 0..count {
        match test_bitmap(vol, start + i, &mut rd) {
            Some(false) => {}
            _ => return false,
        }
    }
    true
}

/// Zero one whole cluster. Fresh clusters are zeroed before the bitmap
/// marks them allocated, so no part of a cluster a file later exposes
/// (a seek gap, a short final write) can show a previous file's bytes.
fn zero_cluster<W: FnMut(u32, &[u8; 512]) -> bool>(
    vol: &ExfatVolume,
    clus: u32,
    mut wr: W,
) -> bool {
    let Some(base) = exfat_ro::cluster_to_lba(vol, clus) else {
        return false;
    };
    let zero = [0u8; 512];
    for s in 0..vol.sec_per_clus {
        if !wr(base + s, &zero) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// File allocation state: where a file's data lives and how much of it
// there is. The redirector keeps one of these per open file (as part
// of its open record); the entry on disk is only rewritten from it at
// persist time.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Alloc {
    pub first_cluster: u32,
    pub no_fat_chain: bool,
    /// Clusters currently allocated to the chain.
    pub clusters: u32,
}

impl Alloc {
    pub const EMPTY: Alloc = Alloc {
        first_cluster: 0,
        no_fat_chain: false,
        clusters: 0,
    };

    pub fn from_entry<R: FnMut(u32, &mut [u8; 512]) -> bool>(
        vol: &ExfatVolume,
        ent: &ExfatEntry,
        rd: R,
    ) -> Alloc {
        Alloc {
            first_cluster: ent.first_cluster,
            no_fat_chain: ent.no_fat_chain,
            clusters: count_chain(vol, ent.first_cluster, ent.no_fat_chain, ent.data_len, rd),
        }
    }

    fn bytes(&self, vol: &ExfatVolume) -> u64 {
        self.clusters as u64 * cluster_bytes(vol)
    }
}

/// How many clusters a chain holds. A contiguous chain's length is
/// arithmetic from the data length (the spec ties the two together);
/// a FAT chain must be walked. The walk is bounded by the volume's
/// cluster count so a cyclic chain on damaged media terminates.
pub fn count_chain<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    first: u32,
    no_fat_chain: bool,
    data_len: u64,
    mut rd: R,
) -> u32 {
    if first == 0 || !valid_cluster(vol, first) {
        return 0;
    }
    if no_fat_chain {
        return (data_len.div_ceil(cluster_bytes(vol))) as u32;
    }
    let mut n = 1u32;
    let mut clus = first;
    while n <= vol.cluster_count {
        match exfat_ro::next_cluster(vol, clus, &mut rd) {
            Some(next) => {
                clus = next;
                n += 1;
            }
            None => break,
        }
    }
    n
}

/// Grow `alloc` until it can hold `need_bytes`. New clusters are
/// zeroed first, marked in the bitmap second, and linked into the FAT
/// last (the module invariant); the directory entry is not touched
/// here at all. A contiguous run is preferred: a fresh file whose
/// whole allocation is one run, and a contiguous file extended by the
/// run that follows it, both keep the NoFatChain form. Anything else
/// becomes (or stays) a FAT chain.
pub fn ensure_capacity<R, W>(
    vol: &ExfatVolume,
    alloc: &mut Alloc,
    need_bytes: u64,
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    let cb = cluster_bytes(vol);
    let need_clusters = need_bytes.div_ceil(cb) as u32;
    while alloc.clusters < need_clusters {
        let want = need_clusters - alloc.clusters;
        // Case 1: the file is a contiguous run (or empty) and the
        // clusters immediately after it can satisfy the whole want.
        if alloc.no_fat_chain || alloc.first_cluster == 0 {
            let start = if alloc.first_cluster == 0 {
                find_free_run(vol, want, &mut rd)
            } else if range_free(vol, alloc.first_cluster + alloc.clusters, want, &mut rd) {
                Some(alloc.first_cluster + alloc.clusters)
            } else {
                None
            };
            if let Some(run) = start {
                for i in 0..want {
                    if !zero_cluster(vol, run + i, &mut wr) {
                        return false;
                    }
                }
                for i in 0..want {
                    if !mark_bitmap(vol, run + i, true, &mut rd, &mut wr) {
                        return false;
                    }
                }
                if alloc.first_cluster == 0 {
                    alloc.first_cluster = run;
                    alloc.no_fat_chain = true;
                }
                alloc.clusters += want;
                continue;
            }
            if alloc.first_cluster != 0 {
                // The run after the file is taken: fall through to the
                // chained case, which converts the file's existing run
                // to FAT links as part of its FAT phase.
            } else {
                // Empty file and no contiguous run anywhere: chained
                // allocation from scratch.
                if !extend_chained(vol, alloc, want, &mut rd, &mut wr) {
                    return false;
                }
                continue;
            }
        }
        // Case 2: FAT-chained growth (including conversion).
        if !extend_chained(vol, alloc, want, &mut rd, &mut wr) {
            return false;
        }
    }
    true
}

/// One round of chained growth: choose up to `want` clusters (a single
/// contiguous run when one exists, else a first-fit batch), zero and
/// mark them, then write every FAT link this round implies — including
/// the internal links of a formerly contiguous file being converted.
fn extend_chained<R, W>(
    vol: &ExfatVolume,
    alloc: &mut Alloc,
    want: u32,
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    // Choose the new clusters. A contiguous run is only taken when
    // the whole run fits the batch buffer; bigger wants fall to the
    // first-fit batch and the caller's loop re-invokes this round for
    // the remainder, so no round ever indexes past the buffer.
    let mut batch = [0u32; 128];
    let (chosen, n): (&[u32], usize) = match find_free_run(vol, want, &mut rd) {
        Some(run) if (want as usize) <= batch.len() => {
            for i in 0..want {
                batch[i as usize] = run + i;
            }
            (&batch, want as usize)
        }
        _ => {
            let cap = core::cmp::min(want as usize, batch.len());
            let got = collect_free(vol, &mut batch[..cap], &mut rd);
            if got == 0 {
                return false;
            }
            (&batch, got)
        }
    };
    // From here on only the first `n` entries of the batch are real
    // clusters; the rest of the buffer is stale scratch.
    let chosen = &chosen[..n];
    let added = n as u32;

    // Data phase: zero the newcomers before anything claims them.
    for &c in chosen {
        if !zero_cluster(vol, c, &mut wr) {
            return false;
        }
    }
    // Bitmap phase.
    for &c in chosen {
        if !mark_bitmap(vol, c, true, &mut rd, &mut wr) {
            return false;
        }
    }
    // FAT phase: conversion links for a formerly contiguous file,
    // the link from the old tail, the batch's internal links, and the
    // end-of-chain mark.
    if alloc.first_cluster == 0 {
        // Fresh chain: links start at the batch head.
    } else if alloc.no_fat_chain {
        for i in 0..alloc.clusters.saturating_sub(1) {
            let c = alloc.first_cluster + i;
            if !put_fat(vol, c, c + 1, &mut rd, &mut wr) {
                return false;
            }
        }
        let tail = alloc.first_cluster + alloc.clusters - 1;
        if !put_fat(vol, tail, chosen[0], &mut rd, &mut wr) {
            return false;
        }
    } else {
        let tail = match chain_cluster_at(vol, alloc.first_cluster, false, alloc.clusters - 1, &mut rd) {
            Some(t) => t,
            None => return false,
        };
        if !put_fat(vol, tail, chosen[0], &mut rd, &mut wr) {
            return false;
        }
    }
    for i in 0..n {
        let next = if i + 1 < n { chosen[i + 1] } else { EOC };
        if !put_fat(vol, chosen[i], next, &mut rd, &mut wr) {
            return false;
        }
    }
    if alloc.first_cluster == 0 {
        alloc.first_cluster = chosen[0];
    }
    alloc.no_fat_chain = false;
    alloc.clusters += added;
    true
}

/// Free every cluster of a chain: bitmap bits first, then the FAT
/// entries, walking the chain before clearing the link that leads to
/// the next cluster. Callers invalidate the directory entry before
/// calling this (the deletion order in the module invariant), so a
/// failure here leaks clusters rather than dangling a live entry.
pub fn free_chain<R, W>(vol: &ExfatVolume, alloc: &Alloc, mut rd: R, mut wr: W) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if alloc.first_cluster == 0 || alloc.clusters == 0 {
        return true;
    }
    let mut ok = true;
    if alloc.no_fat_chain {
        for i in 0..alloc.clusters {
            if !mark_bitmap(vol, alloc.first_cluster + i, false, &mut rd, &mut wr) {
                ok = false;
            }
        }
        return ok;
    }
    let mut clus = alloc.first_cluster;
    let mut freed = 0u32;
    while freed < alloc.clusters && valid_cluster(vol, clus) {
        let next = get_fat(vol, clus, &mut rd);
        if !mark_bitmap(vol, clus, false, &mut rd, &mut wr) {
            ok = false;
        }
        if !put_fat(vol, clus, 0, &mut rd, &mut wr) {
            ok = false;
        }
        freed += 1;
        match next {
            Some(v) if v < 0xFFFF_FFF8 && valid_cluster(vol, v) => clus = v,
            _ => break,
        }
    }
    ok
}

// ---------------------------------------------------------------------------
// Data writes with a chain cursor, the mirror of the redirector's
// read_with_cursor: the cursor names the cluster containing stream
// offset cursor.2, so sequential writes never re-walk the chain.
// ---------------------------------------------------------------------------

/// Write `data` at stream offset `pos` of the chain `alloc` describes.
/// Capacity must already cover `pos + data.len()` (see
/// ensure_capacity); writing short of capacity means the chain ended
/// early, which the caller treats as a write fault. Partial sectors
/// are read-modify-written so bytes outside the written range survive.
pub fn write_with_cursor<R, W>(
    vol: &ExfatVolume,
    alloc: &Alloc,
    cursor: &mut (u32, u32, u64),
    pos: u64,
    data: &[u8],
    mut rd: R,
    mut wr: W,
) -> usize
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if data.is_empty() || alloc.first_cluster < 2 || pos >= alloc.bytes(vol) {
        return 0;
    }
    let want = core::cmp::min(data.len() as u64, alloc.bytes(vol) - pos) as usize;
    let clus_bytes = cluster_bytes(vol);

    if cursor.2 > pos || cursor.0 < 2 {
        *cursor = (alloc.first_cluster, 0, 0);
    }
    while cursor.2 + clus_bytes <= pos {
        let next = if alloc.no_fat_chain {
            cursor.0 + 1
        } else {
            match exfat_ro::next_cluster(vol, cursor.0, &mut rd) {
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
        let lba = base_lba + sec_in_clus as u32;
        let n = core::cmp::min(512 - sec_off, want - done);
        if sec_off == 0 && n == 512 {
            sec.copy_from_slice(&data[done..done + 512]);
            if !wr(lba, &sec) {
                break;
            }
        } else {
            if !rd(lba, &mut sec) {
                break;
            }
            sec[sec_off..sec_off + n].copy_from_slice(&data[done..done + n]);
            if !wr(lba, &sec) {
                break;
            }
        }
        done += n;
        if in_clus + n >= clus_bytes as usize && done < want {
            let next = if alloc.no_fat_chain {
                cursor.0 + 1
            } else {
                match exfat_ro::next_cluster(vol, cursor.0, &mut rd) {
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

/// Zero the stream range `[from, to)`. Used for the gap between the
/// old end of file and a write that starts past it: DOS semantics say
/// those bytes read as zeros, and stale cluster contents are not
/// zeros, so the gap is written, not assumed.
pub fn zero_range<R, W>(
    vol: &ExfatVolume,
    alloc: &Alloc,
    from: u64,
    to: u64,
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if from >= to {
        return true;
    }
    let zero = [0u8; 512];
    let mut cursor = (0, 0, 0);
    let mut pos = from;
    while pos < to {
        let hop = core::cmp::min(512, (to - pos) as usize);
        let n = write_with_cursor(vol, alloc, &mut cursor, pos, &zero[..hop], &mut rd, &mut wr);
        if n == 0 {
            return false;
        }
        pos += n as u64;
    }
    true
}

// ---------------------------------------------------------------------------
// Directory surgery. A directory is a FAT chain of 32-byte entries;
// sets are located by a raw walk that assembles them exactly as the
// parser does (same checksum verification), but also reports where
// each set lives and where a new one could go.
// ---------------------------------------------------------------------------

/// Where one entry set lives: the directory chain that holds it, its
/// byte offset in that chain's stream, and its length in entries.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SetLoc {
    pub dir_cluster: u32,
    pub set_off: u64,
    pub set_entries: u32,
}

/// A located entry: the parsed entry plus its own set location and
/// the location of the containing directory's set in *its* parent
/// (`None` when the containing directory is the root).
#[derive(Copy, Clone)]
pub struct Located {
    pub entry: ExfatEntry,
    pub loc: SetLoc,
    pub dir_self: Option<SetLoc>,
}

struct DirScan {
    found: Option<(ExfatEntry, u64, u32)>,
    /// Start offset of the first run of at least `need` inactive
    /// entries, when one exists.
    free_run: Option<u64>,
    /// Offset of the end-of-directory marker, or the stream length
    /// when the chain ends without one.
    end_off: u64,
    stream_len: u64,
    tail_cluster: u32,
}

/// Walk one directory's raw entry stream, assembling live sets with
/// the parser's rules. `target` (one path component) is matched
/// case-insensitively through the volume's up-case rules, exactly as
/// the parser matches it. With `want_meta` false the walk stops at
/// the first match; create operations pass true because they also
/// need the free-space facts.
fn scan_dir<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    dir_cluster: u32,
    target: &[u16],
    need: u32,
    want_meta: bool,
    mut rd: R,
) -> DirScan {
    let mut scan = DirScan {
        found: None,
        free_run: None,
        end_off: 0,
        stream_len: 0,
        tail_cluster: 0,
    };
    if !valid_cluster(vol, dir_cluster) {
        return scan;
    }
    // Set assembly state, mirroring for_each_dir_entry: `remaining`
    // counts the secondaries still owed by the current primary.
    let mut set_bytes = [0u8; 32 * 21];
    let mut set_len = 0usize;
    let mut set_start = 0u64;
    let mut remaining = 0usize;
    let mut mode = 0u8; // 0 = none, 1 = live set, 2 = skip (active), 3 = skip (inactive)
    let mut cur: Option<ExfatEntry> = None;
    let mut name_target = 0usize;
    let mut name_collected = 0usize;
    let mut stream_seen = false;
    // Inactive-run tracking for create's slot reuse.
    let mut run_start = 0u64;
    let mut run_len = 0u32;
    let mut run_open = false;

    let mut sec = [0u8; 512];
    let mut clus = dir_cluster;
    let mut off = 0u64; // stream offset of the entry being examined
    let mut entries_done = false; // terminator seen: only measuring now
    let mut walked = 0u32;
    'outer: loop {
        if !valid_cluster(vol, clus) {
            break;
        }
        let Some(base) = exfat_ro::cluster_to_lba(vol, clus) else {
            break;
        };
        scan.tail_cluster = clus;
        for s in 0..vol.sec_per_clus {
            if !rd(base + s, &mut sec) {
                break 'outer;
            }
            for i in 0..16usize {
                let mut raw = [0u8; 32];
                raw.copy_from_slice(&sec[i * 32..(i + 1) * 32]);
                let entry_off = off;
                off += 32;
                if entries_done {
                    continue;
                }
                let ty = raw[0];
                if remaining == 0 {
                    if ty == 0x00 {
                        scan.end_off = entry_off;
                        entries_done = true;
                        continue;
                    }
                    if ty == 0x81 || ty == 0x82 || ty == 0x83 {
                        run_open = false;
                        run_len = 0;
                        continue;
                    }
                    if ty & 0x40 != 0 {
                        // Stray secondary with no primary.
                        if ty & 0x80 != 0 {
                            run_open = false;
                            run_len = 0;
                        } else {
                            if !run_open {
                                run_open = true;
                                run_start = entry_off;
                                run_len = 0;
                            }
                            run_len += 1;
                            if scan.free_run.is_none() && run_len >= need && need > 0 {
                                scan.free_run = Some(run_start);
                            }
                        }
                        continue;
                    }
                    // A primary: its SecondaryCount says how many
                    // entries follow, live or deleted alike.
                    let count = raw[1] as usize;
                    if count == 0 {
                        // A file set always has at least a stream
                        // entry; a lone primary is skipped singly.
                        if ty & 0x80 != 0 {
                            run_open = false;
                            run_len = 0;
                        } else {
                            if !run_open {
                                run_open = true;
                                run_start = entry_off;
                                run_len = 0;
                            }
                            run_len += 1;
                            if scan.free_run.is_none() && run_len >= need && need > 0 {
                                scan.free_run = Some(run_start);
                            }
                        }
                        continue;
                    }
                    remaining = count;
                    set_start = entry_off;
                    set_len = 0;
                    name_target = 0;
                    name_collected = 0;
                    stream_seen = false;
                    if ty & 0x80 == 0 {
                        // Deleted set: every entry in it is inactive,
                        // so the whole set extends the current run.
                        mode = 3;
                        if !run_open {
                            run_open = true;
                            run_start = entry_off;
                            run_len = 0;
                        }
                        run_len += 1;
                        if scan.free_run.is_none() && run_len >= need && need > 0 {
                            scan.free_run = Some(run_start);
                        }
                        cur = None;
                    } else {
                        run_open = false;
                        run_len = 0;
                        if ty == 0x85 && count <= 20 {
                            mode = 1;
                            set_bytes[0..32].copy_from_slice(&raw);
                            set_len = 32;
                            cur = Some(ExfatEntry {
                                name: [0u16; 128],
                                name_len: 0,
                                name_truncated: false,
                                attr: u16::from_le_bytes([raw[4], raw[5]]),
                                is_dir: raw[4] & 0x10 != 0,
                                first_cluster: 0,
                                data_len: 0,
                                valid_len: 0,
                                no_fat_chain: false,
                                modified: u32::from_le_bytes([
                                    raw[12], raw[13], raw[14], raw[15],
                                ]),
                            });
                        } else {
                            mode = 2;
                            cur = None;
                        }
                    }
                    continue;
                }
                // Inside a set: consume one secondary.
                remaining -= 1;
                if mode == 3 {
                    run_len += 1;
                    if scan.free_run.is_none() && run_len >= need && need > 0 {
                        scan.free_run = Some(run_start);
                    }
                }
                if mode == 1 {
                    if set_len + 32 <= set_bytes.len() {
                        set_bytes[set_len..set_len + 32].copy_from_slice(&raw);
                        set_len += 32;
                    }
                    let mut drop_set = false;
                    if let Some(e) = cur.as_mut() {
                        if !stream_seen {
                            if ty == 0xC0 {
                                stream_seen = true;
                                e.no_fat_chain = raw[1] & 0x02 != 0;
                                name_target = raw[3] as usize;
                                e.valid_len = u64::from_le_bytes([
                                    raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14],
                                    raw[15],
                                ]);
                                e.first_cluster = u32::from_le_bytes([
                                    raw[20], raw[21], raw[22], raw[23],
                                ]);
                                e.data_len = u64::from_le_bytes([
                                    raw[24], raw[25], raw[26], raw[27], raw[28], raw[29], raw[30],
                                    raw[31],
                                ]);
                            } else {
                                drop_set = true;
                            }
                        } else if ty == 0xC1 {
                            for k in 0..15usize {
                                if name_collected >= name_target {
                                    break;
                                }
                                let ch = u16::from_le_bytes([
                                    raw[2 + k * 2],
                                    raw[3 + k * 2],
                                ]);
                                if name_collected < e.name.len() {
                                    e.name[name_collected] = ch;
                                }
                                name_collected += 1;
                            }
                        }
                    }
                    if drop_set {
                        cur = None;
                        mode = 2;
                    }
                }
                if remaining == 0 {
                    if mode == 1 {
                        if let Some(mut e) = cur.take() {
                            let stored = u16::from_le_bytes([set_bytes[2], set_bytes[3]]);
                            let complete = stream_seen && name_collected == name_target;
                            if complete
                                && exfat_ro::set_checksum(&set_bytes[..set_len]) == stored
                            {
                                e.name_truncated = name_target > e.name.len();
                                e.name_len =
                                    core::cmp::min(name_target, e.name.len()) as u8;
                                if scan.found.is_none()
                                    && names_equal(vol, &e.name[..e.name_len as usize], target, &mut rd)
                                {
                                    scan.found =
                                        Some((e, set_start, (set_len / 32) as u32));
                                    if !want_meta {
                                        scan.stream_len = off;
                                        return scan;
                                    }
                                }
                            }
                        }
                    }
                    mode = 0;
                    set_len = 0;
                }
            }
        }
        walked += 1;
        if walked > vol.cluster_count {
            break; // cyclic chain: stop rather than spin
        }
        match exfat_ro::next_cluster(vol, clus, &mut rd) {
            Some(next) => clus = next,
            None => break,
        }
    }
    if !entries_done {
        scan.end_off = off;
    }
    scan.stream_len = off;
    scan
}

/// Case-insensitive name equality through the volume's up-case rules,
/// the same comparison the parser's find performs.
fn names_equal<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    a: &[u16],
    b: &[u16],
    mut rd: R,
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if exfat_ro::upcase_char(vol, a[i], &mut rd) != exfat_ro::upcase_char(vol, b[i], &mut rd)
        {
            return false;
        }
    }
    true
}

/// Take the next path component off `rest`, the way the parser's
/// find does: leading separators are skipped and components are
/// separated by `\` or `/`. Returns the component and what is left
/// of the path after it, or `None` when nothing but separators (or
/// nothing at all) remains. Streaming rather than collecting into a
/// Vec because the target build is no_std and has no allocator.
fn next_component(mut rest: &[u16]) -> Option<(&[u16], &[u16])> {
    while !rest.is_empty() && (rest[0] == b'\\' as u16 || rest[0] == b'/' as u16) {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return None;
    }
    let mut sep = rest.len();
    for i in 0..rest.len() {
        if rest[i] == b'\\' as u16 || rest[i] == b'/' as u16 {
            sep = i;
            break;
        }
    }
    Some((&rest[..sep], &rest[sep..]))
}

/// Locate a path (from the root, UTF-16, `\` or `/` separated) and
/// report the entry plus where its set lives. This is the writer's
/// counterpart to the parser's `find_path`: same matches, but the
/// answer carries the locations the writer needs.
pub fn locate<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    path: &[u16],
    mut rd: R,
) -> Option<Located> {
    let mut rest = path;
    let mut cur_dir = vol.root_cluster;
    let mut cur_self: Option<SetLoc> = None;
    loop {
        let (comp, after) = next_component(rest)?;
        if comp.len() > 128 {
            return None;
        }
        let scan = scan_dir(vol, cur_dir, comp, 0, false, &mut rd);
        let (entry, off, nent) = scan.found?;
        let loc = SetLoc {
            dir_cluster: cur_dir,
            set_off: off,
            set_entries: nent,
        };
        if next_component(after).is_none() {
            return Some(Located {
                entry,
                loc,
                dir_self: cur_self,
            });
        }
        if !entry.is_dir || entry.first_cluster == 0 {
            return None;
        }
        cur_self = Some(loc);
        cur_dir = entry.first_cluster;
        rest = after;
    }
}

/// Resolve a directory path to its first cluster and its own set
/// location (`None` for the root, which has no set of its own).
pub fn locate_dir<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    path: &[u16],
    mut rd: R,
) -> Option<(u32, Option<SetLoc>)> {
    if next_component(path).is_none() {
        return Some((vol.root_cluster, None));
    }
    let located = locate(vol, path, &mut rd)?;
    if !located.entry.is_dir {
        return None;
    }
    Some((located.entry.first_cluster, Some(located.loc)))
}

/// Read bytes from a directory chain's stream.
fn dir_read_bytes<R: FnMut(u32, &mut [u8; 512]) -> bool>(
    vol: &ExfatVolume,
    dir_cluster: u32,
    mut off: u64,
    mut out: &mut [u8],
    mut rd: R,
) -> usize {
    let mut done = 0usize;
    let mut sec = [0u8; 512];
    while !out.is_empty() {
        let Some(lba) = chain_sector_lba(vol, dir_cluster, false, off, &mut rd) else {
            break;
        };
        if !rd(lba, &mut sec) {
            break;
        }
        let so = (off % 512) as usize;
        let n = core::cmp::min(512 - so, out.len());
        out[..n].copy_from_slice(&sec[so..so + n]);
        out = &mut out[n..];
        off += n as u64;
        done += n;
    }
    done
}

/// Write bytes into a directory chain's stream, read-modify-writing
/// every sector the range touches so neighbouring entries survive.
fn dir_write_bytes<R, W>(
    vol: &ExfatVolume,
    dir_cluster: u32,
    mut off: u64,
    mut data: &[u8],
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    while !data.is_empty() {
        let Some(lba) = chain_sector_lba(vol, dir_cluster, false, off, &mut rd) else {
            return false;
        };
        let so = (off % 512) as usize;
        let n = core::cmp::min(512 - so, data.len());
        if so == 0 && n == 512 {
            let mut sec = [0u8; 512];
            sec.copy_from_slice(&data[..512]);
            if !wr(lba, &sec) {
                return false;
            }
        } else if !rmw_sector(lba, &mut rd, &mut wr, |sec| {
            sec[so..so + n].copy_from_slice(&data[..n]);
        }) {
            return false;
        }
        data = &data[n..];
        off += n as u64;
    }
    true
}

/// Create an empty file's entry set in a directory and return its
/// location. Inactive slots are reused first; otherwise the set is
/// appended at the end of the directory, extending the chain by one
/// cluster when nothing fits. The new file owns no clusters and has
/// length zero — allocation happens on the first write, so the set
/// written here is complete and consistent on its own (the module
/// invariant's "entry last" step for a file that has no data yet).
pub fn create_entry<R, W>(
    vol: &ExfatVolume,
    dir_cluster: u32,
    dir_self: Option<SetLoc>,
    name: &[u16],
    attr: u16,
    timestamp: u32,
    mut rd: R,
    mut wr: W,
) -> Option<SetLoc>
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if name.is_empty() || name.len() > 128 {
        return None;
    }
    let name_entries = name.len().div_ceil(15) as u32;
    let set_entries = 2 + name_entries;
    let set_size = (set_entries * 32) as u64;

    let scan = scan_dir(vol, dir_cluster, name, set_entries, true, &mut rd);
    if scan.found.is_some() {
        return None; // already exists: the caller decides what that means
    }

    let mut extended = false;
    let target_off;
    if let Some(off) = scan.free_run {
        target_off = off;
    } else if scan.end_off + set_size <= scan.stream_len {
        target_off = scan.end_off;
    } else {
        // Extend the directory by one cluster: zero it (data), mark
        // it (bitmap), link it (FAT), then record the new length in
        // the directory's own stream entry before the new set lands
        // in the space — each step only describes space the previous
        // steps have already made real.
        let new_clus = find_free_run(vol, 1, &mut rd)?;
        if !zero_cluster(vol, new_clus, &mut wr) {
            return None;
        }
        if !mark_bitmap(vol, new_clus, true, &mut rd, &mut wr) {
            return None;
        }
        if scan.tail_cluster == 0 {
            return None;
        }
        if !put_fat(vol, scan.tail_cluster, new_clus, &mut rd, &mut wr) {
            return None;
        }
        if !put_fat(vol, new_clus, EOC, &mut rd, &mut wr) {
            return None;
        }
        if let Some(self_loc) = dir_self {
            let new_len = scan.stream_len + cluster_bytes(vol);
            if !patch_stream(
                vol,
                &self_loc,
                None,
                Some(new_len),
                &mut rd,
                &mut wr,
            ) {
                return None;
            }
        }
        extended = true;
        target_off = scan.end_off;
        // Note: if end_off sat mid-chain (a terminator in an earlier
        // cluster), the set is written there and simply spans into
        // the new cluster through the stream addressing.
    }

    // Build the set.
    let mut set = [0u8; 32 * 21];
    let total = set_size as usize;
    set[0] = 0x85;
    set[1] = (set_entries - 1) as u8;
    set[4..6].copy_from_slice(&attr.to_le_bytes());
    set[8..12].copy_from_slice(&timestamp.to_le_bytes());
    set[12..16].copy_from_slice(&timestamp.to_le_bytes());
    set[16..20].copy_from_slice(&timestamp.to_le_bytes());
    let s = 32;
    set[s] = 0xC0;
    // GeneralSecondaryFlags: no allocation yet, so AllocationPossible
    // is clear and FirstCluster/DataLength below are zero, as the
    // spec requires of an empty file.
    set[s + 1] = 0;
    set[s + 3] = name.len() as u8;
    let hash = exfat_ro::name_hash(vol, name, &mut rd);
    set[s + 4..s + 6].copy_from_slice(&hash.to_le_bytes());
    for (i, chunk) in name.chunks(15).enumerate() {
        let n = 64 + i * 32;
        set[n] = 0xC1;
        for (j, &ch) in chunk.iter().enumerate() {
            set[n + 2 + j * 2..n + 4 + j * 2].copy_from_slice(&ch.to_le_bytes());
        }
    }
    let sum = exfat_ro::set_checksum(&set[..total]);
    set[2..4].copy_from_slice(&sum.to_le_bytes());

    if !dir_write_bytes(vol, dir_cluster, target_off, &set[..total], &mut rd, &mut wr) {
        return None;
    }
    // A fresh terminator after an appended set keeps the directory
    // well-formed for every other reader; a reused slot already sits
    // inside a well-formed stream and needs none. The terminator is
    // written only when stream space remains after the set — a set
    // that ends exactly at the end of the (possibly just-extended)
    // chain needs none, because readers stop at the chain end too.
    if scan.free_run.is_none() {
        let final_len = scan.stream_len + if extended { cluster_bytes(vol) } else { 0 };
        let term_off = target_off + set_size;
        if term_off < final_len {
            let zero = [0u8; 32];
            if !dir_write_bytes(vol, dir_cluster, term_off, &zero, &mut rd, &mut wr) {
                return None;
            }
        }
    }
    Some(SetLoc {
        dir_cluster,
        set_off: target_off,
        set_entries,
    })
}

/// Rewrite a set's stream-extension fields (and optionally its
/// attributes) in place, recomputing the set checksum. This is the
/// "entry last" step of every write sequence: it runs only after the
/// data, bitmap, and FAT writes it describes are already on the
/// medium. `stream` carries (first_cluster, valid_len, data_len,
/// no_fat_chain); pass `None` to leave the stream entry's allocation
/// fields untouched (a pure attribute or directory-length update —
/// directory lengths live in the same fields, so a length-only patch
/// passes the current allocation values back; see the length form
/// below).
pub fn patch_stream<R, W>(
    vol: &ExfatVolume,
    loc: &SetLoc,
    stream: Option<(u32, u64, u64, bool)>,
    dir_len: Option<u64>,
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    let mut set = [0u8; 32 * 21];
    let total = (loc.set_entries * 32) as usize;
    if total == 0 || total > set.len() {
        return false;
    }
    if dir_read_bytes(vol, loc.dir_cluster, loc.set_off, &mut set[..total], &mut rd) != total {
        return false;
    }
    if set[0] != 0x85 || set[32] != 0xC0 {
        return false; // the set moved or was replaced: refuse to guess
    }
    let s = 32;
    if let Some((first, valid, len, no_fat_chain)) = stream {
        set[s + 1] = (if first != 0 { 0x01 } else { 0x00 }) | (if no_fat_chain { 0x02 } else { 0x00 });
        set[s + 8..s + 16].copy_from_slice(&valid.to_le_bytes());
        set[s + 20..s + 24].copy_from_slice(&first.to_le_bytes());
        set[s + 24..s + 32].copy_from_slice(&len.to_le_bytes());
    } else if let Some(len) = dir_len {
        set[s + 8..s + 16].copy_from_slice(&len.to_le_bytes());
        set[s + 24..s + 32].copy_from_slice(&len.to_le_bytes());
    } else {
        return false;
    }
    let sum = exfat_ro::set_checksum(&set[..total]);
    set[2..4].copy_from_slice(&sum.to_le_bytes());
    dir_write_bytes(vol, loc.dir_cluster, loc.set_off, &set[..total], &mut rd, &mut wr)
}

/// Truncate a file to empty: the stream entry is zeroed first (so no
/// instant leaves the entry describing clusters that are about to be
/// freed), then the chain is released.
pub fn truncate_file<R, W>(
    vol: &ExfatVolume,
    loc: &SetLoc,
    alloc: &Alloc,
    mut rd: R,
    mut wr: W,
) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    if !patch_stream(vol, loc, Some((0, 0, 0, false)), None, &mut rd, &mut wr) {
        return false;
    }
    free_chain(vol, alloc, &mut rd, &mut wr)
}

/// Mark a set inactive (every entry's type byte loses its in-use
/// bit). The caller frees the file's clusters after this returns —
/// entry first, clusters second, per the module invariant.
pub fn delete_entry<R, W>(vol: &ExfatVolume, loc: &SetLoc, mut rd: R, mut wr: W) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    for i in 0..loc.set_entries {
        let off = loc.set_off + (i * 32) as u64;
        let Some(lba) = chain_sector_lba(vol, loc.dir_cluster, false, off, &mut rd) else {
            return false;
        };
        let so = (off % 512) as usize;
        let ok = rmw_sector(lba, &mut rd, &mut wr, |sec| {
            sec[so] &= 0x7F;
        });
        if !ok {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Volume dirty flag. The VBR's VolumeFlags word (offset 106) carries
// VolumeDirty in bit 1; a volume left dirty tells the next mounter the
// previous session did not close cleanly. The boot checksum excludes
// bytes 106/107 precisely so this flag can flip without invalidating
// the boot region, and the backup boot sector at part_lba + 12 is
// kept in step. PercentInUse (byte 112) is likewise excluded and
// likewise informational; it is not maintained here.
// ---------------------------------------------------------------------------

pub fn set_volume_dirty<R, W>(vol: &ExfatVolume, dirty: bool, mut rd: R, mut wr: W) -> bool
where
    R: FnMut(u32, &mut [u8; 512]) -> bool,
    W: FnMut(u32, &[u8; 512]) -> bool,
{
    let mut ok = true;
    for base in [vol.part_lba, vol.part_lba + 12] {
        let one = rmw_sector(base, &mut rd, &mut wr, |sec| {
            if dirty {
                sec[106] |= 0x02;
            } else {
                sec[106] &= !0x02;
            }
        });
        ok = ok && one;
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exfat_ro;
    use std::cell::RefCell;

    // Synthetic image in the exfat_ro fixture's geometry (512-byte
    // sectors and clusters, FAT at 24, heap at 28) so the two test
    // suites describe the same world. Cluster map: 2 bitmap, 3
    // up-case, 4 root, 5 GAMES, 6..=12 reserved-used filler, 13..=16
    // free. The root carries a deleted set ahead of the GAMES set so
    // slot reuse has something to reuse; GAMES comes in two variants —
    // one with room, one packed with live sets and no terminator.
    const FAT_OFF: usize = 24;
    const HEAP_OFF: usize = 28;
    const CLUSTERS: u32 = 15;

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
        // Same construction as the parser fixture's write_set: the
        // image is built with the parser's own expectations, so writer
        // tests start from a volume the parser already trusts.
        let name16 = ascii_u16(name);
        let secondaries = 1 + (name16.len() + 14) / 15;
        d[at] = 0x85;
        d[at + 1] = secondaries as u8;
        w16(d, at + 4, attr);
        w32(d, at + 8, modified);
        w32(d, at + 12, modified);
        w32(d, at + 16, modified);
        let s = at + 32;
        d[s] = 0xC0;
        d[s + 1] = stream_flags;
        d[s + 3] = name16.len() as u8;
        let mut up = Vec::new();
        for &ch in &name16 {
            up.extend_from_slice(&ch.to_le_bytes());
        }
        // ASCII names up-case to themselves here (fixture names are
        // already uppercase), so the hash input is the name itself.
        w16(d, s + 4, exfat_ro::name_hash_upcased(&up));
        w64(d, s + 8, data_len);
        w32(d, s + 20, first_cluster);
        w64(d, s + 24, data_len);
        for (i, chunk) in name16.chunks(15).enumerate() {
            let n = at + 64 + i * 32;
            d[n] = 0xC1;
            for (j, &ch) in chunk.iter().enumerate() {
                w16(d, n + 2 + j * 2, ch);
            }
        }
        let total = 32 * (1 + secondaries);
        let sum = exfat_ro::set_checksum(&d[at..at + total]);
        w16(d, at + 2, sum);
        at + total
    }

    fn write_deleted_set(d: &mut [u8], at: usize) -> usize {
        d[at] = 0x05;
        d[at + 1] = 2;
        d[at + 32] = 0x40;
        d[at + 64] = 0x41;
        at + 96
    }

    fn build_image(games_full: bool) -> Vec<u8> {
        let mut d = vec![0u8; (HEAP_OFF + CLUSTERS as usize + 2) * 512];
        // VBR
        d[0] = 0xEB;
        d[1] = 0x76;
        d[2] = 0x90;
        d[3..11].copy_from_slice(b"EXFAT   ");
        w64(&mut d, 72, (HEAP_OFF + CLUSTERS as usize) as u64);
        w32(&mut d, 80, FAT_OFF as u32);
        w32(&mut d, 84, 4);
        w32(&mut d, 88, HEAP_OFF as u32);
        w32(&mut d, 92, CLUSTERS);
        w32(&mut d, 96, 4);
        w32(&mut d, 100, 0x1234_5678);
        w16(&mut d, 104, 0x0100);
        d[108] = 9;
        d[109] = 0;
        d[110] = 1;
        d[510] = 0x55;
        d[511] = 0xAA;
        let sum = exfat_ro::boot_checksum(&d[0..11 * 512]);
        for i in 0..128 {
            w32(&mut d, 11 * 512 + i * 4, sum);
        }
        // FAT: system chains end immediately; the rest is unallocated.
        let f = FAT_OFF * 512;
        for c in 0..6u32 {
            w32(&mut d, f + (c as usize) * 4, 0xFFFF_FFFF);
        }
        w32(&mut d, f, 0xFFFF_FFF8);
        // Bitmap: clusters 2..=12 used, 13..=16 free (bits 0..=10 set).
        let bm = coff(2);
        d[bm] = 0xFF;
        d[bm + 1] = 0x07;
        // Up-case table (same compressed form as the parser fixture).
        let uc = coff(3);
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
            let b = d[uc + i];
            table_sum = ((table_sum >> 1) | ((table_sum & 1) << 31)).wrapping_add(b as u32);
        }
        // Root: bitmap, up-case, label, deleted set, GAMES, terminator.
        let ts = (46u32 << 25) | (11 << 21) | (24 << 16) | (3 << 11);
        let mut at = coff(4);
        d[at] = 0x81;
        w32(&mut d, at + 20, 2);
        w64(&mut d, at + 24, 2);
        at += 32;
        d[at] = 0x82;
        w32(&mut d, at + 4, table_sum);
        w32(&mut d, at + 20, 3);
        w64(&mut d, at + 24, upcase_len);
        at += 32;
        d[at] = 0x83;
        d[at + 1] = 6;
        for (i, &ch) in ascii_u16("FERRIC").iter().enumerate() {
            w16(&mut d, at + 2 + i * 2, ch);
        }
        at += 32;
        at = write_deleted_set(&mut d, at);
        let games_len: u64 = if games_full { 512 } else { 512 };
        at = write_set(&mut d, at, "GAMES", 0x10, 0x01, 5, games_len, ts);
        let _ = at;
        // GAMES contents.
        let mut g = coff(5);
        if games_full {
            // Five live sets filling all 16 entries: four 3-entry
            // sets and one 4-entry set (an 18-char name), no
            // terminator, chain ends. Nothing is reusable and nothing
            // is appendable: a create must extend the chain.
            g = write_set(&mut d, g, "DOOM.TXT", 0x20, 0x01, 0, 0, ts);
            g = write_set(&mut d, g, "QUAKE.TXT", 0x20, 0x01, 0, 0, ts);
            g = write_set(&mut d, g, "DUKE.TXT", 0x20, 0x01, 0, 0, ts);
            g = write_set(&mut d, g, "WOLF.TXT", 0x20, 0x01, 0, 0, ts);
            g = write_set(&mut d, g, "LONGFILENAME16.TXT", 0x20, 0x01, 0, 0, ts);
            assert_eq!(g, coff(5) + 512);
        } else {
            g = write_set(&mut d, g, "DOOM.TXT", 0x20, 0x01, 0, 0, ts);
            let _ = g;
        }
        d
    }

    struct Disk {
        img: RefCell<Vec<u8>>,
    }

    impl Disk {
        fn rd(&self) -> impl FnMut(u32, &mut [u8; 512]) -> bool + '_ {
            |lba, out| {
                let img = self.img.borrow();
                let off = lba as usize * 512;
                if off + 512 <= img.len() {
                    out.copy_from_slice(&img[off..off + 512]);
                    true
                } else {
                    false
                }
            }
        }
        fn wr(&self) -> impl FnMut(u32, &[u8; 512]) -> bool + '_ {
            |lba, data| {
                let mut img = self.img.borrow_mut();
                let off = lba as usize * 512;
                if off + 512 <= img.len() {
                    img[off..off + 512].copy_from_slice(data);
                    true
                } else {
                    false
                }
            }
        }
        fn vol(&self) -> ExfatVolume {
            exfat_ro::mount_volume(self.rd()).expect("fixture mounts")
        }
        fn free(&self, vol: &ExfatVolume) -> u32 {
            exfat_ro::free_clusters(vol, self.rd()).expect("free count")
        }
    }

    const TS: u32 = (1 << 21) | (1 << 16); // 1980-01-01 00:00:00

    #[test]
    fn create_write_and_parser_readback() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        assert_eq!(disk.free(&vol), 4);
        // Create reuses the deleted set's slot in the root (offset 96:
        // three system entries precede it).
        let loc = create_entry(&vol, vol.root_cluster, None, &ascii_u16("NEWFILE.TXT"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create");
        assert_eq!(loc.set_off, 96);
        // Allocate and write 600 bytes spanning two clusters.
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 600, disk.rd(), disk.wr()));
        assert_eq!(alloc.clusters, 2);
        assert!(alloc.no_fat_chain, "a single free run stays contiguous");
        let payload: Vec<u8> = (0..600u32).map(|i| (i * 3 + 1) as u8).collect();
        let mut cursor = (0, 0, 0);
        let n = write_with_cursor(&vol, &alloc, &mut cursor, 0, &payload, disk.rd(), disk.wr());
        assert_eq!(n, 600);
        assert!(patch_stream(
            &vol,
            &loc,
            Some((alloc.first_cluster, 600, 600, alloc.no_fat_chain)),
            None,
            disk.rd(),
            disk.wr()
        ));
        // The parser — sharing no code with the writer — reads it back.
        let ent = exfat_ro::find_path(&vol, &ascii_u16("NEWFILE.TXT"), disk.rd()).expect("parser finds it");
        assert_eq!(ent.data_len, 600);
        assert!(ent.no_fat_chain);
        let mut buf = [0u8; 600];
        let n = exfat_ro::read_entry_data(&vol, &ent, 0, &mut buf, disk.rd());
        assert_eq!(n, 600);
        assert_eq!(&buf[..], &payload[..]);
        assert_eq!(disk.free(&vol), 2);
        // The writer's own locate agrees on the location.
        let located = locate(&vol, &ascii_u16("newfile.txt"), disk.rd()).expect("locate");
        assert_eq!(located.loc, loc);
    }

    #[test]
    fn fragmented_bitmap_forces_a_fat_chain() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        // Occupy cluster 14 so the free clusters 13, 15, 16 are not a
        // run of three.
        assert!(mark_bitmap(&vol, 14, true, disk.rd(), disk.wr()));
        let loc = create_entry(&vol, vol.root_cluster, None, &ascii_u16("SCAT.BIN"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create");
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 1300, disk.rd(), disk.wr()));
        assert_eq!(alloc.clusters, 3);
        assert!(!alloc.no_fat_chain);
        assert_eq!(alloc.first_cluster, 13);
        // The chain links 13 -> 15 -> 16.
        assert_eq!(get_fat(&vol, 13, disk.rd()), Some(15));
        assert_eq!(get_fat(&vol, 15, disk.rd()), Some(16));
        assert_eq!(get_fat(&vol, 16, disk.rd()), Some(EOC));
        let payload: Vec<u8> = (0..1300u32).map(|i| (i * 7 + 5) as u8).collect();
        let mut cursor = (0, 0, 0);
        assert_eq!(
            write_with_cursor(&vol, &alloc, &mut cursor, 0, &payload, disk.rd(), disk.wr()),
            1300
        );
        assert!(patch_stream(
            &vol,
            &loc,
            Some((alloc.first_cluster, 1300, 1300, false)),
            None,
            disk.rd(),
            disk.wr()
        ));
        let ent = exfat_ro::find_path(&vol, &ascii_u16("SCAT.BIN"), disk.rd()).expect("parser finds it");
        let mut buf = [0u8; 1300];
        assert_eq!(exfat_ro::read_entry_data(&vol, &ent, 0, &mut buf, disk.rd()), 1300);
        assert_eq!(&buf[..], &payload[..]);
    }

    #[test]
    fn truncate_then_rewrite_leaves_no_stale_tail() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        let loc = create_entry(&vol, vol.root_cluster, None, &ascii_u16("OVER.TXT"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create");
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 600, disk.rd(), disk.wr()));
        let payload: Vec<u8> = vec![0xAA; 600];
        let mut cursor = (0, 0, 0);
        assert_eq!(write_with_cursor(&vol, &alloc, &mut cursor, 0, &payload, disk.rd(), disk.wr()), 600);
        assert!(patch_stream(&vol, &loc, Some((alloc.first_cluster, 600, 600, true)), None, disk.rd(), disk.wr()));
        assert_eq!(disk.free(&vol), 2);
        // Truncate: entry zeroed first, then the chain is released.
        let located = locate(&vol, &ascii_u16("OVER.TXT"), disk.rd()).unwrap();
        let old = Alloc::from_entry(&vol, &located.entry, disk.rd());
        assert!(truncate_file(&vol, &loc, &old, disk.rd(), disk.wr()));
        assert_eq!(disk.free(&vol), 4);
        let ent = exfat_ro::find_path(&vol, &ascii_u16("OVER.TXT"), disk.rd()).unwrap();
        assert_eq!(ent.data_len, 0);
        assert_eq!(ent.first_cluster, 0);
        // Rewrite shorter: exactly the new bytes are visible.
        let mut alloc2 = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc2, 88, disk.rd(), disk.wr()));
        let payload2: Vec<u8> = (0..88u32).map(|i| (i + 40) as u8).collect();
        let mut cursor = (0, 0, 0);
        assert_eq!(write_with_cursor(&vol, &alloc2, &mut cursor, 0, &payload2, disk.rd(), disk.wr()), 88);
        assert!(patch_stream(&vol, &loc, Some((alloc2.first_cluster, 88, 88, true)), None, disk.rd(), disk.wr()));
        let ent = exfat_ro::find_path(&vol, &ascii_u16("OVER.TXT"), disk.rd()).unwrap();
        assert_eq!(ent.data_len, 88);
        let mut buf = [0u8; 200];
        let n = exfat_ro::read_entry_data(&vol, &ent, 0, &mut buf, disk.rd());
        assert_eq!(n, 88);
        assert_eq!(&buf[..88], &payload2[..]);
    }

    #[test]
    fn delete_returns_clusters_and_hides_the_file() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        let loc = create_entry(&vol, vol.root_cluster, None, &ascii_u16("GONE.TXT"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create");
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 600, disk.rd(), disk.wr()));
        assert!(patch_stream(&vol, &loc, Some((alloc.first_cluster, 600, 600, true)), None, disk.rd(), disk.wr()));
        assert_eq!(disk.free(&vol), 2);
        // The redirector's order: entry invalidated first, chain freed
        // second.
        assert!(delete_entry(&vol, &loc, disk.rd(), disk.wr()));
        assert!(exfat_ro::find_path(&vol, &ascii_u16("GONE.TXT"), disk.rd()).is_none());
        assert!(free_chain(&vol, &alloc, disk.rd(), disk.wr()));
        assert_eq!(disk.free(&vol), 4);
    }

    #[test]
    fn create_inside_a_subdirectory() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        let (dir_cluster, dir_self) =
            locate_dir(&vol, &ascii_u16("GAMES"), disk.rd()).expect("locate_dir");
        assert_eq!(dir_cluster, 5);
        let loc = create_entry(&vol, dir_cluster, dir_self, &ascii_u16("NEW2.TXT"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create in subdir");
        assert_eq!(loc.dir_cluster, 5);
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 10, disk.rd(), disk.wr()));
        let payload = b"0123456789";
        let mut cursor = (0, 0, 0);
        assert_eq!(write_with_cursor(&vol, &alloc, &mut cursor, 0, payload, disk.rd(), disk.wr()), 10);
        assert!(patch_stream(&vol, &loc, Some((alloc.first_cluster, 10, 10, true)), None, disk.rd(), disk.wr()));
        let ent = exfat_ro::find_path(&vol, &ascii_u16("GAMES\\NEW2.TXT"), disk.rd())
            .expect("parser finds subdir file");
        let mut buf = [0u8; 10];
        assert_eq!(exfat_ro::read_entry_data(&vol, &ent, 0, &mut buf, disk.rd()), 10);
        assert_eq!(&buf, payload);
    }

    #[test]
    fn full_directory_extends_its_chain() {
        let disk = Disk {
            img: RefCell::new(build_image(true)),
        };
        let vol = disk.vol();
        let (dir_cluster, dir_self) =
            locate_dir(&vol, &ascii_u16("GAMES"), disk.rd()).expect("locate_dir");
        let free_before = disk.free(&vol);
        let loc = create_entry(&vol, dir_cluster, dir_self, &ascii_u16("NEW3.TXT"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create must extend the full directory");
        assert_eq!(loc.dir_cluster, 5);
        // One cluster went to the directory's growth.
        assert_eq!(disk.free(&vol), free_before - 1);
        // The directory's own entry now describes two clusters.
        let games = exfat_ro::find_path(&vol, &ascii_u16("GAMES"), disk.rd()).unwrap();
        assert_eq!(games.data_len, 1024);
        // Every pre-existing file is still there, plus the new one.
        for name in ["DOOM.TXT", "QUAKE.TXT", "DUKE.TXT", "WOLF.TXT", "LONGFILENAME16.TXT", "NEW3.TXT"] {
            let p = format!("GAMES\\{name}");
            assert!(
                exfat_ro::find_path(&vol, &ascii_u16(&p), disk.rd()).is_some(),
                "parser still finds {name}"
            );
        }
    }

    #[test]
    fn seek_gap_is_zero_filled() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        let loc = create_entry(&vol, vol.root_cluster, None, &ascii_u16("GAP.BIN"), 0x20, TS, disk.rd(), disk.wr())
            .expect("create");
        let mut alloc = Alloc::EMPTY;
        assert!(ensure_capacity(&vol, &mut alloc, 100, disk.rd(), disk.wr()));
        let head: Vec<u8> = vec![0x55; 100];
        let mut cursor = (0, 0, 0);
        assert_eq!(write_with_cursor(&vol, &alloc, &mut cursor, 0, &head, disk.rd(), disk.wr()), 100);
        // Extend to a write at 600, zeroing the gap as the redirector
        // does before the data lands.
        assert!(ensure_capacity(&vol, &mut alloc, 650, disk.rd(), disk.wr()));
        assert!(zero_range(&vol, &alloc, 100, 600, disk.rd(), disk.wr()));
        let tail: Vec<u8> = vec![0x77; 50];
        let mut cursor = (0, 0, 0);
        assert_eq!(write_with_cursor(&vol, &alloc, &mut cursor, 600, &tail, disk.rd(), disk.wr()), 50);
        assert!(patch_stream(&vol, &loc, Some((alloc.first_cluster, 650, 650, alloc.no_fat_chain)), None, disk.rd(), disk.wr()));
        let ent = exfat_ro::find_path(&vol, &ascii_u16("GAP.BIN"), disk.rd()).unwrap();
        let mut buf = [0u8; 650];
        assert_eq!(exfat_ro::read_entry_data(&vol, &ent, 0, &mut buf, disk.rd()), 650);
        assert!(buf[..100].iter().all(|&b| b == 0x55));
        assert!(buf[100..600].iter().all(|&b| b == 0), "gap reads as zeros");
        assert!(buf[600..].iter().all(|&b| b == 0x77));
    }

    #[test]
    fn volume_dirty_flag_flips_without_breaking_the_boot_checksum() {
        let disk = Disk {
            img: RefCell::new(build_image(false)),
        };
        let vol = disk.vol();
        assert!(set_volume_dirty(&vol, true, disk.rd(), disk.wr()));
        {
            let img = disk.img.borrow();
            assert_eq!(img[106] & 0x02, 0x02, "main VBR dirty bit set");
            assert_eq!(img[12 * 512 + 106] & 0x02, 0x02, "backup VBR dirty bit set");
        }
        // The volume still mounts: the boot checksum excludes the
        // VolumeFlags bytes by design.
        let vol2 = disk.vol();
        assert_eq!(vol2.serial, vol.serial);
        assert!(set_volume_dirty(&vol, false, disk.rd(), disk.wr()));
        let img = disk.img.borrow();
        assert_eq!(img[106] & 0x02, 0);
    }
}
