# Filesystems — EXFAT.JLM and NTFS.JLM

Two standalone JLMs that give DOS real exFAT and NTFS volumes.
They are the model citizens of this repository: each depends only
on the shared ABI crate and the core's block-service table, and
neither knows or cares which transport sits underneath.

| Module | DDB | Crate | Access |
| --- | --- | --- | --- |
| `EXFAT.JLM` | `0xFD02` | `crates/ferric-exfat` | Read/write |
| `NTFS.JLM` | `0xFD03` | `crates/ferric-ntfs` | Read-only, by construction |

## How they plug in

Both modules consume **only** the core's six-entry block-service
table (device `0xFD01`), reached through Jemm's native `INT 20h`
Get_DDB service-table mechanism. The versioned binary contract is
`crates/abi/src/fs_abi.rs`, whose header states the design intent:
filesystem modules and block transports "can live in separate
public repositories while sharing one versioned binary contract."
Filesystem detection is a core verb (`0x0035`); **FSLOAD.COM**
loads the right module for the detected volume.

## EXFAT.JLM

- `exfat_ro.rs` — the read path (parser core).
- `exfat_rw.rs` — the write path: file WRITE/CREATE/DELETE, with
  the ordering discipline the format demands (data clusters →
  bitmap → FAT → directory entry set last). MKDIR/RENAME/SETATTR
  are refused in this version.
- `redir.rs` — an INT 2Fh redirector so DOS itself sees the
  volume as a drive (mounted through **XREDIR.COM**).

**Redirector caveat:** the redirector is pinned against the
FreeDOS kernel's SDA/SFT layout — a kernel-build-specific
dependency, not a Ferric-core one. It is verified against the
kernel build the parent project ships; a different FreeDOS kernel
build may lay those structures out differently.

## NTFS.JLM

- `ntfs_ro.rs` — a read-only NTFS parser (MFT, runlists,
  resident and non-resident attributes). Read-only is a permanent
  design decision, not a missing feature: an unjournalled writer
  risks Windows "repairing" the volume by truncating files, and
  correct NTFS writing needs $LogFile transaction handling plus
  B-tree index rebalancing.

## Clients

In `crates/cbprobe`: **FSLOAD.COM** (detect + load),
**XREDIR.COM** (redirector mount/unmount), **EXFAT.COM** and
**NTFS.COM** (diagnostics).

## Provenance

The exFAT code was written from Microsoft's published exFAT
specification (2019); the NTFS reader's format knowledge comes
from the ntfs-3g / linux-ntfs corpus. Neither is code-copied —
see `NOTICES.md`.
