#!/usr/bin/env python3
"""Gate 0-2 read-only storage guardrail scanner for ferric-dos.

Mechanically scans target source code (crates/, asm/, etc. - excluding scripts/
host image builders, vendor/, and test fixtures) AND disassembled target
objects/binaries for prohibited Gates 0-2 storage write paths:
  1. BIOS INT 13h write functions: AH in {03h, 05h, 06h, 07h, 0Bh, 0Fh, 43h}
  2. ATA/IDE write command codes: 0x30..0x3F (incl. 0x35, 0x39), 0xCA, 0xCB,
     0xE7, 0xEA in ATA/IDE context or sent to ATA command ports
  3. MBR/FAT modification routines

Exit codes:
  0 - Clean (zero violations)
  1 - One or more violations found
  2 - Usage or tool execution error
"""

from __future__ import annotations

import argparse
import dataclasses
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable, List, Optional, Sequence, Set, Tuple

# Prohibited BIOS INT 13h AH function numbers (Gates 0-2 read-only guardrail).
FORBIDDEN_INT13_AH: Set[int] = {0x03, 0x05, 0x06, 0x07, 0x0B, 0x0F, 0x43}

# Prohibited ATA/IDE write and cache-flush command codes.
FORBIDDEN_ATA_CMDS: Set[int] = set(range(0x30, 0x40)) | {0xCA, 0xCB, 0xE7, 0xEA}

# Standard primary/secondary/tertiary/quaternary ATA command/status I/O ports.
ATA_CMD_PORTS: Set[int] = {0x1F7, 0x177, 0x1E7, 0x167}

# Default target source directories to scan when no paths are passed.
DEFAULT_SCAN_DIRS: Tuple[str, ...] = ("crates", "asm", "kernel", "src")

# Directories excluded by default (host image builders, vendored refs, build outputs).
DEFAULT_EXCLUDED_DIRS: Set[str] = {
    ".git",
    "scripts",
    "vendor",
    "docs",
    "target",
    "out",
    "__pycache__",
}

SOURCE_EXTENSIONS: Set[str] = {".rs", ".asm", ".s", ".S", ".inc", ".c", ".h"}
BINARY_EXTENSIONS: Set[str] = {".o", ".elf", ".a", ".com", ".bin", ".exe"}

# Regexes for parsing numeric literals in Intel asm / C / Rust.
HEX_LITERAL_RE = re.compile(
    r"(?i)(?:0x([0-9a-f]+)|(?<![0-9a-z_])([0-9][0-9a-f]*)h(?![0-9a-z_]))"
)
INT_INSTR_RE = re.compile(r"(?i)\bint\s+(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b")
MOV_AH_RE = re.compile(
    r"(?i)\bmov\s+(?:byte\s*(?:ptr\s*)?)?ah\s*,\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
MOV_AX_EAX_RE = re.compile(
    r"(?i)\bmov\s+(?:(?:word|dword)\s*(?:ptr\s*)?)?(?:e?ax)\s*,\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
MOV_AL_RE = re.compile(
    r"(?i)\bmov\s+(?:byte\s*(?:ptr\s*)?)?al\s*,\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
MOV_DX_EDX_RE = re.compile(
    r"(?i)\bmov\s+(?:(?:word|dword)\s*(?:ptr\s*)?)?(?:e?dx)\s*,\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
OUT_DX_AL_RE = re.compile(r"(?i)\bout\s+dx\s*,\s*al\b")
OUT_IMM_AL_RE = re.compile(
    r"(?i)\bout\s+(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\s*,\s*al\b"
)
CONTROL_FLOW_RESET_RE = re.compile(r"(?i)\b(?:ret|retf|iret|iretd|jmp|call)\b")
AX_CLOBBER_RE = re.compile(r"(?i)\b(?:xor|sub|add|and|or|pop|in)\s+(?:e?ax|ah)\b")
AL_CLOBBER_RE = re.compile(r"(?i)\b(?:xor|sub|add|and|or|pop|in)\s+(?:e?ax|al)\b")
DX_CLOBBER_RE = re.compile(r"(?i)\b(?:xor|sub|add|and|or|pop|in)\s+(?:e?dx)\b")

# C/Rust port I/O helpers writing to ATA command ports.
PORT_OUT_CALL_RE = re.compile(
    r"(?i)\b(?:outb|outp|out_u8|port_write_u8|write_port_u8|io_outb)\s*\(\s*"
    r"(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\s*,\s*"
    r"(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\s*\)"
)

# C/Rust INT 13h wrapper or register setup patterns.
C_REGS_AH_RE = re.compile(
    r"(?i)\b(?:regs?|r)\.(?:[hx]\.)?ah\s*=\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
C_REGS_AX_RE = re.compile(
    r"(?i)\b(?:regs?|r)\.(?:[hx]\.)?ax\s*=\s*(0x[0-9a-f]+|[0-9][0-9a-f]*h|\d+)\b"
)
C_INT13_CALL_RE = re.compile(
    r"(?i)\b(?:int86|int86x|int386|bios_int|biosdisk)\s*\(\s*(?:0x13|19)\b"
)

# ATA/IDE write command names and contextual hex definitions.
ATA_WRITE_SYMBOL_RE = re.compile(
    r"(?i)\b(?:ATA|IDE)_(?:CMD_)?(?:WRITE(?:_[A-Z0-9_]+)?|FLUSH_CACHE(?:_EXT)?|"
    r"DMA_WRITE(?:_[A-Z0-9_]+)?)\b"
)
ATA_CONTEXT_RE = re.compile(
    r"(?i)\b(?:ata|ide|pata|pio_write|dma_write|ata_cmd|ide_cmd|0x1f7|0x177|0x1e7|0x167)\b"
)
ATA_CONST_ASSIGN_RE = re.compile(
    r"(?i)\b(?:const|let|static|#define|equ)\b[^;\n]*\b"
    r"(?:ata|ide|cmd|command|write|flush)[a-z0-9_]*\b[^;\n]*?"
    r"(0x[0-9a-f]+|[0-9][0-9a-f]*h)\b"
)

# MBR / FAT modification routine patterns (code & symbol names).
MBR_FAT_WRITE_RE = re.compile(
    r"(?i)\b(?:"
    r"(?:write|modify|update|patch|erase|wipe|format|corrupt|overwrite)_(?:mbr|fat(?:12|16|32)?|bpb|boot_sector|partition(?:_table)?|lba_sector|disk_sector|sectors?)"
    r"|(?:mbr|fat(?:12|16|32)?|bpb|partition_table)_(?:write|modify|update|patch|format|set_entry|alloc_cluster|free_cluster)"
    r"|mkfs_fat"
    r"|int13_write(?:_sectors|_ext)?"
    r"|bios_write_sectors?"
    r")\b"
)

# Objdump line parser: "  12:   b4 03       mov    ah,0x3"
OBJDUMP_INSN_RE = re.compile(
    r"^\s*([0-9a-fA-F]+):\s+(?:[0-9a-fA-F]{2}\s+)+\s*(.+?)\s*$"
)
OBJDUMP_SYM_RE = re.compile(r"^[0-9a-fA-F]+\s+<([^>]+)>:\s*$")


@dataclasses.dataclass(frozen=True)
class Violation:
    path: str
    line: int
    rule: str
    detail: str

    def format(self) -> str:
        return f"{self.path}:{self.line}: [{self.rule}] {self.detail}"


def parse_int_literal(token: str) -> Optional[int]:
    token = token.strip().lower().replace("_", "")
    try:
        if token.startswith("0x"):
            return int(token, 16)
        if token.endswith("h"):
            return int(token[:-1], 16)
        return int(token, 10)
    except ValueError:
        return None


def strip_comments_for_source(line: str, ext: str) -> str:
    """Remove single-line comments while preserving string literals (for Rust asm!)."""
    if ext in {".asm", ".inc", ".s", ".S"}:
        # Strip ';' comments outside quotes.
        in_sq = False
        in_dq = False
        for i, ch in enumerate(line):
            if ch == "'" and not in_dq:
                in_sq = not in_sq
            elif ch == '"' and not in_sq:
                in_dq = not in_dq
            elif ch == ";" and not in_sq and not in_dq:
                return line[:i]
        return line
    # For Rust/C/H: strip '//' outside double quotes.
    in_dq = False
    escaped = False
    for i in range(len(line) - 1):
        ch = line[i]
        if escaped:
            escaped = False
            continue
        if ch == "\\" and in_dq:
            escaped = True
            continue
        if ch == '"':
            in_dq = not in_dq
            continue
        if not in_dq and ch == "/" and line[i + 1] == "/":
            return line[:i]
    return line


def scan_instruction_stream(
    lines: Iterable[Tuple[int, str]], path_label: str
) -> List[Violation]:
    """Scan a sequence of (line_or_addr, text) assembly/disassembly lines."""
    violations: List[Violation] = []
    # Track recent register immediate values: (value, line_no, instructions_ago)
    recent_ah: Optional[Tuple[int, int, int]] = None
    recent_al: Optional[Tuple[int, int, int]] = None
    recent_dx: Optional[Tuple[int, int, int]] = None
    window = 16

    for line_no, raw_text in lines:
        text = raw_text.strip()
        if not text:
            continue

        # Age tracked register values.
        if recent_ah is not None:
            recent_ah = (recent_ah[0], recent_ah[1], recent_ah[2] + 1)
            if recent_ah[2] > window:
                recent_ah = None
        if recent_al is not None:
            recent_al = (recent_al[0], recent_al[1], recent_al[2] + 1)
            if recent_al[2] > window:
                recent_al = None
        if recent_dx is not None:
            recent_dx = (recent_dx[0], recent_dx[1], recent_dx[2] + 1)
            if recent_dx[2] > window:
                recent_dx = None

        # Check register loads.
        m_ah = MOV_AH_RE.search(text)
        if m_ah:
            val = parse_int_literal(m_ah.group(1))
            if val is not None:
                recent_ah = (val & 0xFF, line_no, 0)

        m_ax = MOV_AX_EAX_RE.search(text)
        if m_ax:
            val = parse_int_literal(m_ax.group(1))
            if val is not None:
                recent_ah = ((val >> 8) & 0xFF, line_no, 0)
                recent_al = (val & 0xFF, line_no, 0)

        m_al = MOV_AL_RE.search(text)
        if m_al:
            val = parse_int_literal(m_al.group(1))
            if val is not None:
                recent_al = (val & 0xFF, line_no, 0)

        m_dx = MOV_DX_EDX_RE.search(text)
        if m_dx:
            val = parse_int_literal(m_dx.group(1))
            if val is not None:
                recent_dx = (val & 0xFFFF, line_no, 0)

        # Check C/Rust reg struct assignments.
        m_c_ah = C_REGS_AH_RE.search(text)
        if m_c_ah:
            val = parse_int_literal(m_c_ah.group(1))
            if val is not None:
                recent_ah = (val & 0xFF, line_no, 0)

        m_c_ax = C_REGS_AX_RE.search(text)
        if m_c_ax:
            val = parse_int_literal(m_c_ax.group(1))
            if val is not None:
                recent_ah = ((val >> 8) & 0xFF, line_no, 0)
                recent_al = (val & 0xFF, line_no, 0)

        # Check INT 13h calls (assembly or C wrapper).
        m_int = INT_INSTR_RE.search(text)
        if m_int:
            vec = parse_int_literal(m_int.group(1))
            if vec == 0x13 and recent_ah is not None:
                ah_val, ah_line, _ = recent_ah
                if ah_val in FORBIDDEN_INT13_AH:
                    violations.append(
                        Violation(
                            path=path_label,
                            line=line_no,
                            rule="INT13_WRITE",
                            detail=(
                                f"BIOS INT 13h with prohibited write function "
                                f"AH=0x{ah_val:02X} (loaded at line {ah_line}): {text}"
                            ),
                        )
                    )
            # Any INT resets tracked AH/AL for subsequent instructions.
            recent_ah = None
            recent_al = None

        if C_INT13_CALL_RE.search(text) and recent_ah is not None:
            ah_val, ah_line, _ = recent_ah
            if ah_val in FORBIDDEN_INT13_AH:
                violations.append(
                    Violation(
                        path=path_label,
                        line=line_no,
                        rule="INT13_WRITE",
                        detail=(
                            f"BIOS INT 13h call with prohibited write function "
                            f"AH=0x{ah_val:02X} (set at line {ah_line}): {text}"
                        ),
                    )
                )
            recent_ah = None

        # Check ATA command port writes (OUT dx, al / OUT imm, al).
        if OUT_DX_AL_RE.search(text):
            if (
                recent_dx is not None
                and recent_dx[0] in ATA_CMD_PORTS
                and recent_al is not None
                and recent_al[0] in FORBIDDEN_ATA_CMDS
            ):
                violations.append(
                    Violation(
                        path=path_label,
                        line=line_no,
                        rule="ATA_WRITE_CMD",
                        detail=(
                            f"ATA write command 0x{recent_al[0]:02X} (line {recent_al[1]}) "
                            f"written to ATA command port 0x{recent_dx[0]:03X} "
                            f"(line {recent_dx[1]}): {text}"
                        ),
                    )
                )

        m_out_imm = OUT_IMM_AL_RE.search(text)
        if m_out_imm:
            port = parse_int_literal(m_out_imm.group(1))
            if (
                port in ATA_CMD_PORTS
                and recent_al is not None
                and recent_al[0] in FORBIDDEN_ATA_CMDS
            ):
                violations.append(
                    Violation(
                        path=path_label,
                        line=line_no,
                        rule="ATA_WRITE_CMD",
                        detail=(
                            f"ATA write command 0x{recent_al[0]:02X} (line {recent_al[1]}) "
                            f"written to ATA command port 0x{port:03X}: {text}"
                        ),
                    )
                )

        # Clobbers / control flow boundaries.
        if CONTROL_FLOW_RESET_RE.search(text):
            recent_ah = None
            recent_al = None
            recent_dx = None
        else:
            if not m_ah and not m_ax and AX_CLOBBER_RE.search(text):
                recent_ah = None
            if not m_al and not m_ax and AL_CLOBBER_RE.search(text):
                recent_al = None
            if not m_dx and DX_CLOBBER_RE.search(text):
                recent_dx = None

    return violations


def scan_source_file(path: Path) -> List[Violation]:
    """Scan a single source file (.rs, .asm, .c, .h, etc.)."""
    violations: List[Violation] = []
    ext = path.suffix
    try:
        raw_lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as exc:
        raise RuntimeError(f"Failed to read {path}: {exc}") from exc

    stripped_lines: List[Tuple[int, str]] = []
    in_block_comment = False

    for idx, raw in enumerate(raw_lines, start=1):
        line = raw
        if ext in {".rs", ".c", ".h"}:
            # Handle /* ... */ block comments simply.
            cleaned_chars: List[str] = []
            i = 0
            while i < len(line):
                if in_block_comment:
                    if i + 1 < len(line) and line[i] == "*" and line[i + 1] == "/":
                        in_block_comment = False
                        i += 2
                        continue
                    i += 1
                    continue
                if i + 1 < len(line) and line[i] == "/" and line[i + 1] == "*":
                    in_block_comment = True
                    i += 2
                    continue
                cleaned_chars.append(line[i])
                i += 1
            line = "".join(cleaned_chars)

        code = strip_comments_for_source(line, ext).strip()
        if not code:
            continue
        stripped_lines.append((idx, code))

        # Rule 2a: Direct port I/O helper call writing forbidden ATA command.
        for m_out in PORT_OUT_CALL_RE.finditer(code):
            port = parse_int_literal(m_out.group(1))
            val = parse_int_literal(m_out.group(2))
            if port in ATA_CMD_PORTS and val in FORBIDDEN_ATA_CMDS:
                violations.append(
                    Violation(
                        path=str(path),
                        line=idx,
                        rule="ATA_WRITE_CMD",
                        detail=(
                            f"Port I/O write of prohibited ATA command 0x{val:02X} "
                            f"to ATA command port 0x{port:03X}: {code}"
                        ),
                    )
                )

        # Rule 2b: Named ATA/IDE write command symbols.
        m_ata_sym = ATA_WRITE_SYMBOL_RE.search(code)
        if m_ata_sym:
            violations.append(
                Violation(
                    path=str(path),
                    line=idx,
                    rule="ATA_WRITE_SYMBOL",
                    detail=f"Prohibited ATA/IDE write symbol '{m_ata_sym.group(0)}': {code}",
                )
            )
        else:
            # Rule 2c: ATA/IDE constant assignment with forbidden write command code.
            m_const = ATA_CONST_ASSIGN_RE.search(code)
            if m_const and ATA_CONTEXT_RE.search(code):
                val = parse_int_literal(m_const.group(1))
                if val in FORBIDDEN_ATA_CMDS:
                    violations.append(
                        Violation(
                            path=str(path),
                            line=idx,
                            rule="ATA_WRITE_CMD",
                            detail=(
                                f"Prohibited ATA/IDE write command code 0x{val:02X} "
                                f"in ATA/IDE context: {code}"
                            ),
                        )
                    )

        # Rule 3: MBR/FAT modification routines.
        m_mbr = MBR_FAT_WRITE_RE.search(code)
        if m_mbr:
            violations.append(
                Violation(
                    path=str(path),
                    line=idx,
                    rule="MBR_FAT_WRITE",
                    detail=(
                        f"Prohibited MBR/FAT/storage modification routine "
                        f"'{m_mbr.group(0)}': {code}"
                    ),
                )
            )

    # Rule 1 + asm Rule 2: instruction-level INT 13h and OUT tracking.
    violations.extend(scan_instruction_stream(stripped_lines, str(path)))
    return violations


def disassemble_binary(
    path: Path, raw_mode: Optional[str] = None, objdump: str = "objdump"
) -> str:
    if raw_mode == "16":
        cmd = [objdump, "-D", "-b", "binary", "-m", "i8086", "-M", "intel", str(path)]
    elif raw_mode == "32":
        cmd = [objdump, "-D", "-b", "binary", "-m", "i386", "-M", "intel", str(path)]
    else:
        cmd = [objdump, "-d", "-M", "intel", str(path)]

    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    except FileNotFoundError as exc:
        raise RuntimeError(f"'{objdump}' not found on PATH") from exc

    if proc.returncode != 0:
        raise RuntimeError(
            f"{' '.join(cmd)} failed (rc={proc.returncode}): {proc.stderr.strip()}"
        )
    return proc.stdout


def scan_binary_file(
    path: Path, raw_mode: Optional[str] = None, objdump: str = "objdump"
) -> List[Violation]:
    """Disassemble a target object/binary and scan instructions & symbols."""
    disasm = disassemble_binary(path, raw_mode=raw_mode, objdump=objdump)
    violations: List[Violation] = []
    insn_lines: List[Tuple[int, str]] = []

    for line_idx, raw_line in enumerate(disasm.splitlines(), start=1):
        m_sym = OBJDUMP_SYM_RE.match(raw_line)
        if m_sym:
            sym = m_sym.group(1)
            m_mbr = MBR_FAT_WRITE_RE.search(sym)
            if m_mbr:
                violations.append(
                    Violation(
                        path=str(path),
                        line=line_idx,
                        rule="MBR_FAT_WRITE_SYMBOL",
                        detail=f"Prohibited MBR/FAT write symbol <{sym}> in binary",
                    )
                )
            continue

        m_insn = OBJDUMP_INSN_RE.match(raw_line)
        if m_insn:
            insn = m_insn.group(2)
            # Strip trailing objdump '# ...' comments.
            if "#" in insn:
                insn = insn.split("#", 1)[0].strip()
            insn_lines.append((line_idx, insn))

    violations.extend(scan_instruction_stream(insn_lines, str(path)))
    return violations


def collect_files(
    roots: Sequence[Path], excluded_dirs: Set[str]
) -> Tuple[List[Path], List[Path]]:
    """Collect (source_files, binary_files) under the given paths."""
    source_files: List[Path] = []
    binary_files: List[Path] = []

    for root in roots:
        if not root.exists():
            raise RuntimeError(f"Path does not exist: {root}")
        if root.is_file():
            if root.suffix in BINARY_EXTENSIONS:
                binary_files.append(root)
            else:
                source_files.append(root)
            continue

        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = sorted(d for d in dirnames if d not in excluded_dirs)
            for fname in sorted(filenames):
                fpath = Path(dirpath) / fname
                if fpath.suffix in SOURCE_EXTENSIONS:
                    source_files.append(fpath)
                elif fpath.suffix in BINARY_EXTENSIONS:
                    binary_files.append(fpath)

    return source_files, binary_files


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Ferric DOS Gate 0-2 read-only storage guardrail scanner."
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help="Files or directories to scan (default: crates/ and asm/ if present).",
    )
    parser.add_argument(
        "--disasm",
        action="append",
        default=[],
        help="Explicit target object/binary file to disassemble and scan (repeatable).",
    )
    parser.add_argument(
        "--raw-16",
        action="store_true",
        help="Disassemble --disasm / binary inputs as raw 16-bit x86 (i8086) binaries.",
    )
    parser.add_argument(
        "--raw-32",
        action="store_true",
        help="Disassemble --disasm / binary inputs as raw 32-bit x86 (i386) binaries.",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        help="Additional directory name to exclude (repeatable).",
    )
    parser.add_argument(
        "--objdump",
        default="objdump",
        help="Path to objdump executable (default: objdump).",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Print each scanned file.",
    )
    args = parser.parse_args(argv)

    if args.raw_16 and args.raw_32:
        print("error: --raw-16 and --raw-32 are mutually exclusive", file=sys.stderr)
        return 2

    raw_mode = "16" if args.raw_16 else ("32" if args.raw_32 else None)
    excluded = set(DEFAULT_EXCLUDED_DIRS) | set(args.exclude)

    if args.paths:
        roots = [Path(p) for p in args.paths]
    elif args.disasm:
        roots = []
    else:
        roots = [Path(d) for d in DEFAULT_SCAN_DIRS if Path(d).exists()]

    try:
        source_files, binary_files = collect_files(roots, excluded)
        for dpath in args.disasm:
            p = Path(dpath)
            if not p.exists():
                raise RuntimeError(f"Disassembly target does not exist: {p}")
            binary_files.append(p)
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    all_violations: List[Violation] = []
    try:
        for sf in source_files:
            if args.verbose:
                print(f"[scan-src] {sf}")
            all_violations.extend(scan_source_file(sf))
        for bf in binary_files:
            if args.verbose:
                print(f"[scan-bin] {bf}")
            all_violations.extend(
                scan_binary_file(bf, raw_mode=raw_mode, objdump=args.objdump)
            )
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if all_violations:
        for v in all_violations:
            print(v.format(), file=sys.stderr)
        print(
            f"FAIL: read-only guardrail found {len(all_violations)} violation(s) "
            f"across {len(source_files)} source and {len(binary_files)} binary file(s).",
            file=sys.stderr,
        )
        return 1

    print(
        f"PASS: read-only guardrail clean "
        f"({len(source_files)} source, {len(binary_files)} binary file(s) scanned)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
