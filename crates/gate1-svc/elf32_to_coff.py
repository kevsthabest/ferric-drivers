#!/usr/bin/env python3
"""Convert a merged 32-bit x86 ELF relocatable object (.text/.rodata/.data/.bss
with R_386_32 and R_386_PC32 relocations only) into a standard i386 Win32 COFF
object (.obj) suitable for OpenWatcom wlink PE32 JLM linking.
"""

from __future__ import annotations

import struct
import sys
from typing import Dict, List, Tuple

# ELF32 constants
SHT_PROGBITS = 1
SHT_SYMTAB = 2
SHT_STRTAB = 3
SHT_NOBITS = 8
SHT_REL = 9

R_386_32 = 1
R_386_PC32 = 2

STB_LOCAL = 0
STB_GLOBAL = 1
STT_SECTION = 3

# Win32 COFF constants
IMAGE_FILE_MACHINE_I386 = 0x014C
IMAGE_SCN_CNT_CODE = 0x00000020
IMAGE_SCN_CNT_INITIALIZED_DATA = 0x00000040
IMAGE_SCN_CNT_UNINITIALIZED_DATA = 0x00000080
IMAGE_SCN_ALIGN_4BYTES = 0x00300000
IMAGE_SCN_MEM_EXECUTE = 0x20000000
IMAGE_SCN_MEM_READ = 0x40000000
IMAGE_SCN_MEM_WRITE = 0x80000000

IMAGE_REL_I386_DIR32 = 0x0006
IMAGE_REL_I386_REL32 = 0x0014

IMAGE_SYM_CLASS_EXTERNAL = 2
IMAGE_SYM_CLASS_STATIC = 3


def read_cstr(buf: bytes, off: int) -> str:
    end = buf.index(b"\x00", off)
    return buf[off:end].decode("ascii")


def convert_elf32_to_coff(elf_bytes: bytes) -> bytes:
    if elf_bytes[:4] != b"\x7fELF" or elf_bytes[4] != 1 or elf_bytes[5] != 1:
        raise ValueError("Input must be 32-bit little-endian ELF")

    (
        _e_type,
        _e_machine,
        _e_ver,
        _e_entry,
        _e_phoff,
        e_shoff,
        _e_flags,
        _e_ehsize,
        _e_phentsize,
        _e_phnum,
        e_shentsize,
        e_shnum,
        e_shstrndx,
    ) = struct.unpack_from("<HHIIIIIHHHHHH", elf_bytes, 16)

    shdrs = []
    for i in range(e_shnum):
        sh = struct.unpack_from("<IIIIIIIIII", elf_bytes, e_shoff + i * e_shentsize)
        shdrs.append(sh)

    shstr_off = shdrs[e_shstrndx][4]
    shstr_sz = shdrs[e_shstrndx][5]
    shstrtab = elf_bytes[shstr_off : shstr_off + shstr_sz]

    symtab_idx = None
    strtab_bytes = b""
    for idx, sh in enumerate(shdrs):
        if sh[1] == SHT_SYMTAB:
            symtab_idx = idx
            str_sh = shdrs[sh[6]]
            strtab_bytes = elf_bytes[str_sh[4] : str_sh[4] + str_sh[5]]
            break
    if symtab_idx is None:
        raise ValueError("ELF object has no .symtab")

    # Select allocatable sections (.text, .rodata, .data, .bss)
    coff_secs: List[Dict] = []
    elf_to_coff_sec: Dict[int, int] = {}

    for idx, sh in enumerate(shdrs):
        name = read_cstr(shstrtab, sh[0])
        sh_type, sh_flags, _addr, sh_off, sh_size = sh[1], sh[2], sh[3], sh[4], sh[5]
        if sh_type not in (SHT_PROGBITS, SHT_NOBITS) or (sh_flags & 0x2) == 0:
            continue
        if sh_size == 0:
            continue
        if name == ".text":
            chars = IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ | IMAGE_SCN_ALIGN_4BYTES
            raw = bytearray(elf_bytes[sh_off : sh_off + sh_size])
        elif name == ".rodata":
            chars = IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_ALIGN_4BYTES
            raw = bytearray(elf_bytes[sh_off : sh_off + sh_size])
        elif name == ".data":
            chars = (
                IMAGE_SCN_CNT_INITIALIZED_DATA
                | IMAGE_SCN_MEM_READ
                | IMAGE_SCN_MEM_WRITE
                | IMAGE_SCN_ALIGN_4BYTES
            )
            raw = bytearray(elf_bytes[sh_off : sh_off + sh_size])
        elif name == ".bss":
            chars = (
                IMAGE_SCN_CNT_UNINITIALIZED_DATA
                | IMAGE_SCN_MEM_READ
                | IMAGE_SCN_MEM_WRITE
                | IMAGE_SCN_ALIGN_4BYTES
            )
            raw = bytearray()
        else:
            continue
        coff_sec_num = len(coff_secs) + 1  # 1-based in COFF
        elf_to_coff_sec[idx] = coff_sec_num
        coff_secs.append(
            {
                "name": name,
                "elf_idx": idx,
                "size": sh_size,
                "raw": raw,
                "chars": chars,
                "relocs": [],
            }
        )

    # Parse ELF symbols and build COFF symbol table
    sym_sh = shdrs[symtab_idx]
    sym_count = sym_sh[5] // 16
    coff_syms: List[Tuple[str, int, int, int]] = []  # (name, value, sec_num, storage_class)
    elf_to_coff_sym: Dict[int, int] = {}

    # Emit one static symbol per COFF section first (for STT_SECTION relocations)
    sec_sym_idx: Dict[int, int] = {}
    for s_i, sec in enumerate(coff_secs, start=1):
        sec_sym_idx[sec["elf_idx"]] = len(coff_syms)
        coff_syms.append((sec["name"], 0, s_i, IMAGE_SYM_CLASS_STATIC))

    for i in range(1, sym_count):
        st_name, st_value, _st_size, st_info, _st_other, st_shndx = struct.unpack_from(
            "<IIIBBH", elf_bytes, sym_sh[4] + i * 16
        )
        bind = st_info >> 4
        sym_type = st_info & 0xF
        if sym_type == STT_SECTION:
            if st_shndx in sec_sym_idx:
                elf_to_coff_sym[i] = sec_sym_idx[st_shndx]
            continue
        name = read_cstr(strtab_bytes, st_name)
        if not name:
            continue
        if st_shndx == 0:
            sec_num = 0
            sclass = IMAGE_SYM_CLASS_EXTERNAL
        elif st_shndx in elf_to_coff_sec:
            sec_num = elf_to_coff_sec[st_shndx]
            sclass = IMAGE_SYM_CLASS_EXTERNAL if bind == STB_GLOBAL else IMAGE_SYM_CLASS_STATIC
        else:
            continue
        elf_to_coff_sym[i] = len(coff_syms)
        coff_syms.append((name, st_value, sec_num, sclass))

    # Translate .rel.* sections
    for sh in shdrs:
        if sh[1] != SHT_REL:
            continue
        target_elf_sec = sh[7]
        if target_elf_sec not in elf_to_coff_sec:
            continue
        csec = coff_secs[elf_to_coff_sec[target_elf_sec] - 1]
        rel_off, rel_sz = sh[4], sh[5]
        for r_pos in range(0, rel_sz, 8):
            r_offset, r_info = struct.unpack_from("<II", elf_bytes, rel_off + r_pos)
            r_sym = r_info >> 8
            r_type = r_info & 0xFF
            if r_sym not in elf_to_coff_sym:
                raise ValueError(f"Unmapped ELF symbol index {r_sym} in relocation at {r_offset:#x}")
            csym = elf_to_coff_sym[r_sym]
            if r_type == R_386_32:
                csec["relocs"].append((r_offset, csym, IMAGE_REL_I386_DIR32))
            elif r_type in (R_386_PC32, 4):  # R_386_PC32 or R_386_PLT32 (identical in static link)
                # ELF PC32 stores addend relative to r_offset (usually -4);
                # COFF REL32 is relative to r_offset + 4, so add +4 to in-place word.
                cur = struct.unpack_from("<i", csec["raw"], r_offset)[0]
                struct.pack_into("<i", csec["raw"], r_offset, cur + 4)
                csec["relocs"].append((r_offset, csym, IMAGE_REL_I386_REL32))
            else:
                raise ValueError(f"Unsupported ELF32 relocation type {r_type}")

    # Build COFF string table for long symbol names (> 8 bytes)
    strtab = bytearray(4)  # first 4 bytes = total size

    def encode_sym_name(s: str) -> bytes:
        b = s.encode("ascii")
        if len(b) <= 8:
            return b.ljust(8, b"\x00")
        off = len(strtab)
        strtab.extend(b + b"\x00")
        return struct.pack("<II", 0, off)

    # Compute layout offsets
    num_secs = len(coff_secs)
    hdr_size = 20 + num_secs * 40
    cur_off = hdr_size

    sec_headers = bytearray()
    sec_bodies = bytearray()

    for sec in coff_secs:
        raw_ptr = 0
        raw_len = len(sec["raw"])
        if raw_len > 0:
            raw_ptr = cur_off
            sec_bodies.extend(sec["raw"])
            cur_off += raw_len
            pad = (-cur_off) & 3
            if pad:
                sec_bodies.extend(b"\x00" * pad)
                cur_off += pad
        rel_ptr = 0
        num_rel = len(sec["relocs"])
        if num_rel > 0:
            rel_ptr = cur_off
            for r_off, csym, r_type in sec["relocs"]:
                sec_bodies.extend(struct.pack("<IIH", r_off, csym, r_type))
            cur_off += num_rel * 10
        name8 = sec["name"].encode("ascii")[:8].ljust(8, b"\x00")
        sec_headers.extend(
            struct.pack(
                "<8sIIIIIIHHI",
                name8,
                0,
                0,
                sec["size"],
                raw_ptr,
                rel_ptr,
                0,
                num_rel,
                0,
                sec["chars"],
            )
        )

    sym_table_off = cur_off
    sym_table = bytearray()
    for name, val, sec_num, sclass in coff_syms:
        sym_table.extend(encode_sym_name(name))
        sym_table.extend(struct.pack("<IhHBB", val, sec_num, 0, sclass, 0))

    struct.pack_into("<I", strtab, 0, len(strtab))

    file_hdr = struct.pack(
        "<HHIIIHH",
        IMAGE_FILE_MACHINE_I386,
        num_secs,
        0,
        sym_table_off,
        len(coff_syms),
        0,
        0,
    )
    return bytes(file_hdr + sec_headers + sec_bodies + sym_table + strtab)


def main() -> int:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <in.o> <out.obj>", file=sys.stderr)
        return 2
    elf_bytes = open(sys.argv[1], "rb").read()
    coff_bytes = convert_elf32_to_coff(elf_bytes)
    open(sys.argv[2], "wb").write(coff_bytes)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
