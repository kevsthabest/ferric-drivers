# Building

Every module in this repository is built the same way: by its own
per-crate `build.sh`, run from the repository root, inside the pinned
build container. There is no single top-level build command, and a
plain `cargo build` does **not** produce a usable module — it stops
halfway, at a Rust static library that still needs linking,
conversion, assembly, and packaging before Jemm will load it.

## Building one driver at a time

Each row below is one command, run from the repository root inside
the pinned container. The satellite builds read gate1-svc's linker
script and converter from the source tree, so no other module needs
to be built first.

| Command | Produces |
| --- | --- |
| `sh crates/gate1-svc/build.sh` | `out/FERRIC.JLM` — the resident core, which also hosts the Ethernet, Wi-Fi, USB OHCI, and audio drivers; they are not separate modules and cannot be built apart from the core |
| `sh crates/ferric-exfat/build.sh` | `out/EXFAT.JLM` |
| `sh crates/ferric-ntfs/build.sh` | `out/NTFS.JLM` |
| `sh crates/ferric-vbe/build.sh` | `out/VBE.JLM` |
| `sh crates/cbprobe/build.sh` | The `.COM` client tools in `out/` |

Three crates have no build script of their own — `abi`, `wifi-core`,
and `ferric-ehci` are libraries, and they compile in as dependencies
of the builds above.

## Toolchain

- The pinned container image, `ferric-dos-build` (see the
  `Dockerfile` at the repository root).
- Rust **nightly-2026-11-20** (pinned in `rust-toolchain.toml`), with
  the `rust-src` and `llvm-tools` components. The build uses
  `-Z build-std` to build `core` for the custom target, so a stable
  toolchain cannot work.
- The custom target `targets/i386-unknown-none.json` (wired up in
  `.cargo/config.toml`): 32-bit x86, 386 instruction baseline, no
  MMX/SSE, `panic = "abort"`.
- GNU `ld`, NASM, Python 3, and the Open Watcom `wlink` linker.

## What a per-crate build actually does

Using `crates/gate1-svc/build.sh` as the reference (the satellite
modules follow the same shape):

1. Assemble the real-mode stubs with NASM.
2. `cargo build --release -p <crate> --target
   targets/i386-unknown-none.json`, producing a static library.
3. Partial-link with GNU `ld -m elf_i386` using
   `crates/gate1-svc/linker.ld` (for the resident core) or
   `crates/gate1-svc/jlm_merge.ld` (for satellite modules — the
   exFAT/NTFS/VBE builds cross-reference gate1-svc's linker script
   and converter, which is why they live in that crate).
4. Run the 386-purity checker (`scripts/check_386_purity.py`) over
   the linked object: the 386 baseline is a hard rule, and any
   486-or-later instruction in the output fails the build.
5. Convert ELF32 to COFF with `crates/gate1-svc/elf32_to_coff.py`,
   assemble the module's JLM stub with NASM, and link with Open
   Watcom `wlink` into a PE32 DLL.
6. PX-patch the DLL into the final `.JLM` file under `out/`.

Outputs land in `out/` at the repository root (`FERRIC.JLM`,
`EXFAT.JLM`, `NTFS.JLM`, `VBE.JLM`, plus the `.COM` clients from
`crates/cbprobe/build.sh`, which is NASM-only).

## What this repository does not include

Stated plainly, so nobody has to discover it by failing:

- **`scripts/test.sh` and `scripts/build_image.sh` are excluded.**
  In the parent project they drive the full Ferric DOS system image
  (boot files, non-driver crates, QEMU smoke tests). They would fail
  on this subset and are not part of the driver build.
- **`ath5k_initvals.rs` is the shipped artifact.** Its generator,
  `scripts/gen_ath5k_initvals.py`, is included for reference, but it
  regenerates from a vendored Linux ath5k tree that is not part of
  this repository, so it cannot be re-run from this tree alone.
- **`boot/` is excluded.** Those are Ferric DOS system-image
  scripts, not driver build inputs.
- **Jemm/JLOAD and HDPMI32i are not bundled.** They are runtime
  prerequisites (see the README); the parent project's image
  pipeline fetches them separately, and so must any user of these
  drivers.
- The workspace `Cargo.toml` covers only the seven driver crates.
  The parent project's workspace also contains non-driver crates
  (`hwscan`, `ali-probe`, `pwr-probe`) that are not published here.
