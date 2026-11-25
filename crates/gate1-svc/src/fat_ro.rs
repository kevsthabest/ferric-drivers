//! Gate 2.6 — Strictly Read-Only MBR + FAT12/FAT16/FAT32 Parser for USB Mass Storage.
//!
//! Pure `#![no_std]`, zero-heap, 386-pure sector-callback reader. Contains ZERO
//! filesystem or block write logic by construction.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FatKind {
    Fat12,
    Fat16,
    Fat32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct FatVolume {
    pub kind: FatKind,
    pub part_lba: u32,
    pub bytes_per_sec: u16,
    pub sec_per_clus: u8,
    pub fat_lba: u32,
    pub fat_secs: u32,
    pub root_dir_lba: u32,     // FAT12/16 only
    pub root_dir_secs: u32,    // FAT12/16 only
    pub root_cluster: u32,     // FAT32 only
    pub data_lba: u32,
    pub total_clusters: u32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DirEntryInfo {
    pub name83: [u8; 11],
    pub attr: u8,
    pub first_cluster: u32,
    pub file_size: u32,
}

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn r32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Check whether a 512-byte sector looks like a valid FAT BPB (superfloppy or partition VBR).
pub fn parse_bpb(sec: &[u8; 512], part_lba: u32) -> Option<FatVolume> {
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return None;
    }
    let bps = r16(sec, 11);
    let spc = sec[13];
    let rsvd = r16(sec, 14) as u32;
    let num_fats = sec[16] as u32;
    let root_ents = r16(sec, 17) as u32;
    let tot16 = r16(sec, 19) as u32;
    let fat16 = r16(sec, 22) as u32;
    let tot32 = r32(sec, 32);
    let fat32 = r32(sec, 36);

    if bps != 512 || spc == 0 || !spc.is_power_of_two() || rsvd == 0 || num_fats == 0 || num_fats > 4 {
        return None;
    }
    let total_secs = if tot16 != 0 { tot16 } else { tot32 };
    let fat_secs = if fat16 != 0 { fat16 } else { fat32 };
    if total_secs == 0 || fat_secs == 0 {
        return None;
    }

    let root_dir_secs = (root_ents * 32 + 511) / 512;
    let fat_lba = part_lba.checked_add(rsvd)?;
    let root_dir_lba = fat_lba.checked_add(num_fats.checked_mul(fat_secs)?)?;
    let data_lba = root_dir_lba.checked_add(root_dir_secs)?;
    // The data-area overhead must be built with checked arithmetic all the
    // way: in plain u32 math a crafted BPB with a near-maximal FAT size can
    // wrap the sum back to a small number, and the checked_sub below would
    // then happily produce a huge bogus data area (and a wrong FAT kind)
    // instead of rejecting the volume.
    let overhead = rsvd
        .checked_add(num_fats.checked_mul(fat_secs)?)?
        .checked_add(root_dir_secs)?;
    let data_secs = total_secs.checked_sub(overhead)?;
    let total_clusters = data_secs / (spc as u32);
    if total_clusters < 16 {
        return None;
    }

    let (kind, root_cluster) = if fat16 == 0 && root_ents == 0 && total_clusters >= 65525 {
        (FatKind::Fat32, r32(sec, 44) & 0x0FFF_FFFF)
    } else if total_clusters < 4085 {
        (FatKind::Fat12, 0)
    } else {
        (FatKind::Fat16, 0)
    };

    Some(FatVolume {
        kind,
        part_lba,
        bytes_per_sec: bps,
        sec_per_clus: spc,
        fat_lba,
        fat_secs,
        root_dir_lba,
        root_dir_secs,
        root_cluster,
        data_lba,
        total_clusters,
    })
}

/// Inspect sector 0: if it's a valid FAT VBR (superfloppy), mount directly;
/// otherwise scan the 4 MBR partition entries for the first FAT partition (types
/// `0x01, 0x04, 0x06, 0x0B, 0x0C, 0x0E`) and parse its VBR.
pub fn mount_volume<F>(mut read_sec: F) -> Option<FatVolume>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut sec0 = [0u8; 512];
    if !read_sec(0, &mut sec0) {
        return None;
    }
    if sec0[510] != 0x55 || sec0[511] != 0xAA {
        return None;
    }
    // Check MBR partition table first (accepting any non-empty, non-GPT-protective
    // partition whose VBR is a valid FAT12/16/32 BPB — covers 0x01/04/06/0B/0C/0E,
    // 0xEF EFI/Ventoy FAT partitions, and hidden FAT types while skipping exFAT/NTFS).
    for i in 0..4usize {
        let p = 446 + i * 16;
        let ptype = sec0[p + 4];
        let start_lba = r32(&sec0, p + 8);
        let num_secs = r32(&sec0, p + 12);
        if ptype != 0x00 && ptype != 0xEE && start_lba > 0 && num_secs > 0 {
            let mut vbr = [0u8; 512];
            if read_sec(start_lba, &mut vbr) {
                if let Some(vol) = parse_bpb(&vbr, start_lba) {
                    return Some(vol);
                }
            }
        }
    }
    // Fallback: unpartitioned superfloppy FAT image at LBA 0.
    parse_bpb(&sec0, 0)
}

/// Whether `clus` names a data cluster that actually exists on this
/// volume. Cluster numbers come from on-disk FAT and directory data, so a
/// corrupt or crafted image can hand back any u32; without this check an
/// out-of-range value would steer sector reads anywhere on the device and
/// be interpreted as file data or as more FAT.
fn valid_cluster(vol: &FatVolume, clus: u32) -> bool {
    clus >= 2 && clus - 2 < vol.total_clusters
}

/// Convert a cluster number (`>= 2`) to its first LBA on disk.
pub fn cluster_to_lba(vol: &FatVolume, clus: u32) -> Option<u32> {
    if !valid_cluster(vol, clus) {
        return None;
    }
    vol.data_lba.checked_add((clus - 2).checked_mul(vol.sec_per_clus as u32)?)
}

/// Look up the next cluster in the FAT12/FAT16/FAT32 table, returning `None` at EOC or error.
pub fn next_cluster<F>(vol: &FatVolume, clus: u32, mut read_sec: F) -> Option<u32>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    if !valid_cluster(vol, clus) {
        return None;
    }
    let mut sec = [0u8; 512];
    match vol.kind {
        FatKind::Fat12 => {
            let byte_off = clus + (clus / 2);
            let fat_sec = vol.fat_lba + (byte_off / 512);
            let off = (byte_off % 512) as usize;
            if !read_sec(fat_sec, &mut sec) {
                return None;
            }
            let lo = sec[off] as u16;
            let hi = if off + 1 < 512 {
                sec[off + 1] as u16
            } else {
                let mut sec2 = [0u8; 512];
                if !read_sec(fat_sec + 1, &mut sec2) {
                    return None;
                }
                sec2[0] as u16
            };
            let raw = lo | (hi << 8);
            let val = if (clus & 1) != 0 { raw >> 4 } else { raw & 0x0FFF } as u32;
            if val >= 0x0FF8 || !valid_cluster(vol, val) { None } else { Some(val) }
        }
        FatKind::Fat16 => {
            let byte_off = clus * 2;
            let fat_sec = vol.fat_lba + (byte_off / 512);
            let off = (byte_off % 512) as usize;
            if !read_sec(fat_sec, &mut sec) {
                return None;
            }
            let val = r16(&sec, off) as u32;
            if val >= 0xFFF8 || !valid_cluster(vol, val) { None } else { Some(val) }
        }
        FatKind::Fat32 => {
            let byte_off = clus * 4;
            let fat_sec = vol.fat_lba + (byte_off / 512);
            let off = (byte_off % 512) as usize;
            if !read_sec(fat_sec, &mut sec) {
                return None;
            }
            let val = r32(&sec, off) & 0x0FFF_FFFF;
            if val >= 0x0FFF_FFF8 || !valid_cluster(vol, val) { None } else { Some(val) }
        }
    }
}

/// Convert an ASCII 8.3 filename (e.g. `b"README.TXT"`) into uppercase 11-byte
/// space-padded FAT directory format (`b"README  TXT"`).
pub fn encode_83_name(input: &[u8]) -> Option<[u8; 11]> {
    let mut out = [b' '; 11];
    if input.is_empty() {
        return None;
    }
    let mut i = 0usize;
    let mut pos = 0usize;
    let mut in_ext = false;
    while i < input.len() {
        let mut ch = input[i];
        i += 1;
        if ch == b'.' {
            if in_ext || pos == 0 {
                return None;
            }
            in_ext = true;
            pos = 8;
            continue;
        }
        if ch >= b'a' && ch <= b'z' {
            ch -= 32;
        }
        if in_ext {
            if pos >= 11 {
                return None;
            }
        } else if pos >= 8 {
            return None;
        }
        out[pos] = ch;
        pos += 1;
    }
    if out[0] == b' ' {
        return None;
    }
    Some(out)
}

/// Format an 11-byte FAT 8.3 directory name into `out` (returns length written).
pub fn format_83_name(name83: &[u8; 11], out: &mut [u8; 12]) -> usize {
    let mut n = 0usize;
    let mut end_base = 8usize;
    while end_base > 0 && name83[end_base - 1] == b' ' {
        end_base -= 1;
    }
    for i in 0..end_base {
        out[n] = name83[i];
        n += 1;
    }
    let mut end_ext = 11usize;
    while end_ext > 8 && name83[end_ext - 1] == b' ' {
        end_ext -= 1;
    }
    if end_ext > 8 {
        out[n] = b'.';
        n += 1;
        for i in 8..end_ext {
            out[n] = name83[i];
            n += 1;
        }
    }
    n
}

fn parse_dir_sector<C>(sec: &[u8; 512], cb: &mut C) -> bool
where
    C: FnMut(&DirEntryInfo) -> bool,
{
    for i in 0..16usize {
        let e = &sec[i * 32..(i + 1) * 32];
        if e[0] == 0x00 {
            return false; // end of directory
        }
        if e[0] == 0xE5 {
            continue; // deleted
        }
        let attr = e[11];
        if attr == 0x0F || (attr & 0x08) != 0 {
            continue; // LFN or volume label
        }
        let mut name83 = [0u8; 11];
        name83.copy_from_slice(&e[0..11]);
        let clus_hi = r16(e, 20) as u32;
        let clus_lo = r16(e, 26) as u32;
        let info = DirEntryInfo {
            name83,
            attr,
            first_cluster: (clus_hi << 16) | clus_lo,
            file_size: r32(e, 28),
        };
        if !cb(&info) {
            return false;
        }
    }
    true
}

/// Iterate entries in a FAT root directory (or subdirectory starting at `dir_cluster`).
/// Pass `dir_cluster == 0` for the root directory.
pub fn for_each_dir_entry<F, C>(
    vol: &FatVolume,
    dir_cluster: u32,
    mut read_sec: F,
    mut cb: C,
) where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
    C: FnMut(&DirEntryInfo) -> bool,
{
    let mut sec = [0u8; 512];
    if dir_cluster == 0 && vol.kind != FatKind::Fat32 {
        for s in 0..vol.root_dir_secs {
            if !read_sec(vol.root_dir_lba + s, &mut sec) {
                return;
            }
            if !parse_dir_sector(&sec, &mut cb) {
                return;
            }
        }
    } else {
        let mut clus = if dir_cluster == 0 { vol.root_cluster } else { dir_cluster };
        for _ in 0..128 {
            let Some(lba) = cluster_to_lba(vol, clus) else {
                return;
            };
            for s in 0..(vol.sec_per_clus as u32) {
                if !read_sec(lba + s, &mut sec) {
                    return;
                }
                if !parse_dir_sector(&sec, &mut cb) {
                    return;
                }
            }
            match next_cluster(vol, clus, &mut read_sec) {
                Some(next) => clus = next,
                None => return,
            }
        }
    }
}

/// Look up an 8.3 path (e.g. `b"TEST.TXT"` or `b"SUB\\FILE.BIN"`) on `vol`.
pub fn find_path<F>(vol: &FatVolume, path: &[u8], mut read_sec: F) -> Option<DirEntryInfo>
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut cur_dir: u32 = 0;
    let mut rest = path;
    while !rest.is_empty() && (rest[0] == b'\\' || rest[0] == b'/') {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return None;
    }
    loop {
        let mut sep = rest.len();
        for i in 0..rest.len() {
            if rest[i] == b'\\' || rest[i] == b'/' {
                sep = i;
                break;
            }
        }
        let comp = &rest[..sep];
        let want = encode_83_name(comp)?;
        let mut found: Option<DirEntryInfo> = None;
        for_each_dir_entry(vol, cur_dir, &mut read_sec, |e| {
            if e.name83 == want {
                found = Some(*e);
                false
            } else {
                true
            }
        });
        let entry = found?;
        if sep >= rest.len() {
            return Some(entry);
        }
        if (entry.attr & 0x10) == 0 {
            return None; // expected directory
        }
        cur_dir = entry.first_cluster;
        rest = &rest[sep + 1..];
        while !rest.is_empty() && (rest[0] == b'\\' || rest[0] == b'/') {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return Some(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_and_format_83_names() {
        let enc = encode_83_name(b"readme.txt").unwrap();
        assert_eq!(&enc, b"README  TXT");
        let mut buf = [0u8; 12];
        let len = format_83_name(&enc, &mut buf);
        assert_eq!(&buf[..len], b"README.TXT");

        let noext = encode_83_name(b"DOOM").unwrap();
        assert_eq!(&noext, b"DOOM       ");
        let len2 = format_83_name(&noext, &mut buf);
        assert_eq!(&buf[..len2], b"DOOM");
    }

    #[test]
    fn parse_bpb_fat16_and_read_file_via_mock_disk() {
        // Minimal 16-sector FAT16 superfloppy: 1 rsvd, 1 FAT (1 sec), 1 root dir sec, data at LBA 3
        let mut disk = [[0u8; 512]; 16];
        // VBR at LBA 0
        disk[0][11..13].copy_from_slice(&512u16.to_le_bytes()); // bytes/sec
        disk[0][13] = 1; // sec/clus
        disk[0][14..16].copy_from_slice(&1u16.to_le_bytes()); // reserved
        disk[0][16] = 1; // 1 FAT
        disk[0][17..19].copy_from_slice(&16u16.to_le_bytes()); // 16 root entries (1 sec)
        disk[0][19..21].copy_from_slice(&4200u16.to_le_bytes()); // total secs (>4085 -> FAT16)
        disk[0][22..24].copy_from_slice(&1u16.to_le_bytes()); // fat secs
        disk[0][510] = 0x55;
        disk[0][511] = 0xAA;

        // FAT at LBA 1: cluster 2 -> cluster 3 -> EOC (0xFFFF)
        disk[1][4..6].copy_from_slice(&3u16.to_le_bytes());
        disk[1][6..8].copy_from_slice(&0xFFFFu16.to_le_bytes());

        // Root dir at LBA 2: HELLO.TXT, cluster 2, size 11
        disk[2][0..11].copy_from_slice(b"HELLO   TXT");
        disk[2][11] = 0x20;
        disk[2][26..28].copy_from_slice(&2u16.to_le_bytes());
        disk[2][28..32].copy_from_slice(&11u32.to_le_bytes());

        // Cluster 2 at LBA 3: "Hello USB!\n"
        disk[3][0..11].copy_from_slice(b"Hello USB!\n");

        let read_fn = |lba: u32, out: &mut [u8; 512]| -> bool {
            if (lba as usize) < disk.len() {
                *out = disk[lba as usize];
                true
            } else {
                false
            }
        };
        let vol = mount_volume(read_fn).unwrap();
        assert_eq!(vol.kind, FatKind::Fat16);
        assert_eq!(vol.data_lba, 3);
        let ent = find_path(&vol, b"hello.txt", read_fn).unwrap();
        assert_eq!(ent.first_cluster, 2);
        assert_eq!(ent.file_size, 11);
        assert_eq!(next_cluster(&vol, 2, read_fn), Some(3));
        assert_eq!(next_cluster(&vol, 3, read_fn), None);
    }

    #[test]
    fn cluster_numbers_are_validated_against_the_volume() {
        let vol = FatVolume {
            kind: FatKind::Fat16,
            part_lba: 0,
            bytes_per_sec: 512,
            sec_per_clus: 1,
            fat_lba: 1,
            fat_secs: 8,
            root_dir_lba: 9,
            root_dir_secs: 1,
            root_cluster: 0,
            data_lba: 10,
            total_clusters: 100,
        };
        // Clusters run 2..=101 on this volume.
        assert_eq!(cluster_to_lba(&vol, 2), Some(10));
        assert_eq!(cluster_to_lba(&vol, 101), Some(109));
        assert_eq!(cluster_to_lba(&vol, 102), None);
        assert_eq!(cluster_to_lba(&vol, 1), None);
        assert_eq!(cluster_to_lba(&vol, u32::MAX), None);

        // A FAT entry pointing past the end of the volume ends the walk
        // instead of steering the next read to a foreign sector.
        let mut disk = [[0u8; 512]; 16];
        disk[0][11..13].copy_from_slice(&512u16.to_le_bytes());
        disk[0][13] = 1;
        disk[0][14..16].copy_from_slice(&1u16.to_le_bytes());
        disk[0][16] = 1;
        disk[0][17..19].copy_from_slice(&16u16.to_le_bytes());
        disk[0][19..21].copy_from_slice(&4200u16.to_le_bytes());
        disk[0][22..24].copy_from_slice(&1u16.to_le_bytes());
        disk[0][510] = 0x55;
        disk[0][511] = 0xAA;
        // Cluster 2 -> 5000, but this volume only has 4197 clusters.
        disk[1][4..6].copy_from_slice(&5000u16.to_le_bytes());
        let read_fn = |lba: u32, out: &mut [u8; 512]| -> bool {
            if (lba as usize) < disk.len() {
                *out = disk[lba as usize];
                true
            } else {
                false
            }
        };
        let vol = mount_volume(read_fn).unwrap();
        assert_eq!(next_cluster(&vol, 2, read_fn), None);
        assert_eq!(next_cluster(&vol, 5000, read_fn), None);
    }

    #[test]
    fn parse_bpb_rejects_absurd_fat_size() {
        // FAT32-sized fields with a near-maximal FAT length: the data area
        // left over is far below the minimum cluster count, so the volume
        // must be rejected rather than mounted with wrapped geometry.
        let mut sec = [0u8; 512];
        sec[11..13].copy_from_slice(&512u16.to_le_bytes());
        sec[13] = 1;
        sec[14..16].copy_from_slice(&1u16.to_le_bytes());
        sec[16] = 4;
        sec[32..36].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        sec[36..40].copy_from_slice(&0x3FFF_FFFFu32.to_le_bytes());
        sec[510] = 0x55;
        sec[511] = 0xAA;
        assert_eq!(parse_bpb(&sec, 0), None);
    }
}
