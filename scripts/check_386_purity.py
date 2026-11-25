#!/usr/bin/env python3
"""Gate 0 i386 instruction-purity scanner for ferric-dos.

Disassembles target ELF objects/archives/binaries (.o, .elf, .a) or raw 32-bit
binaries (--raw) via `objdump -d -M intel` and fails on any 486+, Pentium+,
686+, fence, MMX/SSE/SSE2/AVX, or x87 FPU instruction in baseline code.

Default mode is strict (zero forbidden instructions allowed). Optional
`--allow-section` and `--allow-symbol` flags allow explicit runtime-guarded
dispatch sections/symbols when needed.

Exit codes:
  0 - Clean (zero forbidden instructions)
  1 - One or more forbidden instructions found
  2 - Usage or tool execution error
"""

from __future__ import annotations

import argparse
import dataclasses
import fnmatch
import re
import subprocess
import sys
from pathlib import Path
from typing import List, Optional, Sequence, Set, Tuple

# Exact forbidden mnemonics (486+, Pentium+, 686+, fences, SSE/MMX control, x87).
FORBIDDEN_EXACT: Set[str] = {
    # 486+
    "bswap",
    "cmpxchg",
    "xadd",
    "invd",
    "wbinvd",
    "invlpg",
    # Pentium (586)+
    "cmpxchg8b",
    "cpuid",
    "rdmsr",
    "wrmsr",
    "rdtsc",
    "rsm",
    # Pentium Pro / II / III (686)+
    "rdpmc",
    "sysenter",
    "sysexit",
    "ud0",
    "ud1",
    "ud2",
    "ud2a",
    "ud2b",
    # Fences / SSE / SSE2 / cache / modern control
    "mfence",
    "lfence",
    "sfence",
    "clflush",
    "clflushopt",
    "clwb",
    "pause",
    "rdtscp",
    "monitor",
    "mwait",
    "xgetbv",
    "xsetbv",
    "fxsave",
    "fxrstor",
    "ldmxcsr",
    "stmxcsr",
    "emms",
    "femms",
    "movq",
    "movd",
    "movdqa",
    "movdqu",
    "movdq2q",
    "movq2dq",
    "movaps",
    "movups",
    "movapd",
    "movupd",
    "movhps",
    "movlps",
    "movhpd",
    "movlpd",
    "movhlps",
    "movlhps",
    "movmskps",
    "movmskpd",
    "movntps",
    "movntpd",
    "movntdq",
    "movntq",
    "movnti",
    "movss",
    "por",
    "pxor",
    "pand",
    "pandn",
    "addps",
    "addss",
    "addpd",
    "addsd",
    "subps",
    "subss",
    "subpd",
    "subsd",
    "mulps",
    "mulss",
    "mulpd",
    "mulsd",
    "divps",
    "divss",
    "divpd",
    "divsd",
    "sqrtps",
    "sqrtss",
    "sqrtpd",
    "sqrtsd",
    "rsqrtps",
    "rsqrtss",
    "rcpps",
    "rcpss",
    "maxps",
    "maxss",
    "maxpd",
    "maxsd",
    "minps",
    "minss",
    "minpd",
    "minsd",
    "andps",
    "andpd",
    "andnps",
    "andnpd",
    "orps",
    "orpd",
    "xorps",
    "xorpd",
    "comiss",
    "comisd",
    "ucomiss",
    "ucomisd",
    "cmpps",
    "cmpss",
    "cmppd",
    "shufps",
    "shufpd",
    "unpckhps",
    "unpcklps",
    "unpckhpd",
    "unpcklpd",
    "maskmovq",
    "maskmovdqu",
    "pmovmskb",
    # x87 FPU (prohibited in Rust baseline code; 387 is optional hardware)
    "f2xm1",
    "fabs",
    "fadd",
    "faddp",
    "fbld",
    "fbstp",
    "fchs",
    "fclex",
    "fnclex",
    "fcom",
    "fcomp",
    "fcompp",
    "fcomi",
    "fcomip",
    "fcos",
    "fdecstp",
    "fdiv",
    "fdivp",
    "fdivr",
    "fdivrp",
    "ffree",
    "fiadd",
    "ficom",
    "ficomp",
    "fidiv",
    "fidivr",
    "fild",
    "fimul",
    "fincstp",
    "finit",
    "fninit",
    "fist",
    "fistp",
    "fisttp",
    "fisub",
    "fisubr",
    "fld",
    "fld1",
    "fldcw",
    "fldenv",
    "fldl2e",
    "fldl2t",
    "fldlg2",
    "fldln2",
    "fldpi",
    "fldz",
    "fmul",
    "fmulp",
    "fnop",
    "fpatan",
    "fprem",
    "fprem1",
    "fptan",
    "frndint",
    "frstor",
    "fsave",
    "fnsave",
    "fscale",
    "fsin",
    "fsincos",
    "fsqrt",
    "fst",
    "fstcw",
    "fnstcw",
    "fstenv",
    "fnstenv",
    "fstp",
    "fstsw",
    "fnstsw",
    "fsub",
    "fsubp",
    "fsubr",
    "fsubrp",
    "ftst",
    "fucom",
    "fucomp",
    "fucompp",
    "fucomi",
    "fucomip",
    "fwait",
    "wait",
    "fxam",
    "fxch",
    "fxtract",
    "fyl2x",
    "fyl2xp1",
}

# Prefix families that are strictly 686+ or MMX/SSE/SSE2.
FORBIDDEN_PREFIXES: Tuple[str, ...] = (
    "cmov",
    "fcmov",
    "prefetch",
    "cvt",
    "padd",
    "psub",
    "pmul",
    "psll",
    "psrl",
    "psra",
    "pack",
    "punpck",
    "pshuf",
    "pavg",
    "pmax",
    "pmin",
    "pcmp",
    "pinsr",
    "pextr",
    "psad",
    "pmadd",
    "setcc",  # not real, just safety; 386 has setcc (sete/setne) which are valid 386
)
# Remove accidental placeholder if any - keep only real prefixes.
FORBIDDEN_PREFIXES = tuple(p for p in FORBIDDEN_PREFIXES if p != "setcc")

# Instruction prefixes that can precede the actual mnemonic on a disassembly line.
X86_PREFIX_TOKENS: Set[str] = {
    "lock",
    "rep",
    "repe",
    "repz",
    "repne",
    "repnz",
    "data16",
    "data32",
    "addr16",
    "addr32",
    "cs",
    "ds",
    "es",
    "fs",
    "gs",
    "ss",
    "notrack",
    "bnd",
}

# SIMD / x87 register references in operands (catches movsd/cmpsd SSE2 vs 386 string!).
FORBIDDEN_REG_RE = re.compile(
    r"(?i)(?:\b(?:mm[0-7]|xmm(?:[0-9]|1[0-5])|ymm(?:[0-9]|1[0-5])|zmm(?:[0-9]|[12][0-9]|3[01]))\b"
    r"|\bst(?:\([0-7]\))?\b)"
)

OBJDUMP_SECTION_RE = re.compile(r"^Disassembly of section ([^:]+):\s*$")
OBJDUMP_SYM_RE = re.compile(r"^[0-9a-fA-F]+\s+<([^>]+)>:\s*$")
OBJDUMP_INSN_RE = re.compile(
    r"^\s*([0-9a-fA-F]+):\s+(?:[0-9a-fA-F]{2}\s+)+\s*(.+?)\s*$"
)


@dataclasses.dataclass(frozen=True)
class PurityViolation:
    path: str
    section: str
    symbol: str
    address: str
    instruction: str
    reason: str

    def format(self) -> str:
        loc = f"{self.path} ({self.section} <{self.symbol}> @ 0x{self.address})"
        return f"{loc}: [{self.reason}] {self.instruction}"


def classify_instruction(insn_text: str) -> Optional[str]:
    """Return a violation reason if insn_text is forbidden on i386, else None."""
    # Strip objdump trailing '# ...' annotation.
    cleaned = insn_text.split("#", 1)[0].strip()
    if not cleaned:
        return None

    parts = cleaned.split(None, 1)
    head = parts[0].lower()
    rest = parts[1] if len(parts) > 1 else ""

    # Consume leading x86 instruction prefixes (rep, lock, data16, etc.).
    while head in X86_PREFIX_TOKENS and rest:
        sub = rest.split(None, 1)
        head = sub[0].lower()
        rest = sub[1] if len(sub) > 1 else ""

    # Register check first: catches SSE2 movsd/cmpsd with xmm, MMX with mm0..7,
    # AVX with ymm/zmm, and x87 st(0)..st(7), while allowing 386 `rep movsd` / `cmpsd`.
    m_reg = FORBIDDEN_REG_RE.search(rest)
    if m_reg:
        return f"SIMD/x87 register '{m_reg.group(0)}'"

    if head in FORBIDDEN_EXACT:
        return f"forbidden mnemonic '{head}'"

    for pref in FORBIDDEN_PREFIXES:
        if head.startswith(pref):
            return f"forbidden mnemonic family '{head}'"

    return None


def disassemble(
    path: Path, raw: bool = False, raw_16: bool = False, objdump: str = "objdump"
) -> str:
    if raw_16:
        cmd = [objdump, "-D", "-b", "binary", "-m", "i8086", "-M", "intel", str(path)]
    elif raw:
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


def scan_disassembly(
    disasm: str,
    path_label: str,
    allow_sections: Sequence[str] = (),
    allow_symbols: Sequence[str] = (),
) -> Tuple[List[PurityViolation], int, int]:
    """Scan objdump output and return (violations, insn_count, skipped_count)."""
    violations: List[PurityViolation] = []
    section = ".text"
    symbol = "<unknown>"
    insn_count = 0
    skipped_count = 0

    for line in disasm.splitlines():
        m_sec = OBJDUMP_SECTION_RE.match(line)
        if m_sec:
            section = m_sec.group(1).strip()
            continue

        m_sym = OBJDUMP_SYM_RE.match(line)
        if m_sym:
            symbol = m_sym.group(1).strip()
            continue

        m_insn = OBJDUMP_INSN_RE.match(line)
        if not m_insn:
            continue

        addr = m_insn.group(1)
        insn = m_insn.group(2).strip()
        insn_count += 1

        if any(fnmatch.fnmatch(section, pat) for pat in allow_sections) or any(
            fnmatch.fnmatch(symbol, pat) for pat in allow_symbols
        ):
            skipped_count += 1
            continue

        reason = classify_instruction(insn)
        if reason is not None:
            violations.append(
                PurityViolation(
                    path=path_label,
                    section=section,
                    symbol=symbol,
                    address=addr,
                    instruction=insn,
                    reason=reason,
                )
            )

    return violations, insn_count, skipped_count


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Ferric DOS Gate 0 i386 instruction-purity scanner."
    )
    parser.add_argument(
        "files",
        nargs="+",
        help="Target ELF object/archive/binary files (.o, .elf, .a) or raw binaries.",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="Disassemble inputs as raw 32-bit x86 (i386) flat binaries.",
    )
    parser.add_argument(
        "--raw-16",
        action="store_true",
        help="Disassemble inputs as raw 16-bit x86 (i8086) flat binaries.",
    )
    parser.add_argument(
        "--allow-section",
        action="append",
        default=[],
        help="Explicit runtime-guarded section glob to allowlist (default: none).",
    )
    parser.add_argument(
        "--allow-symbol",
        action="append",
        default=[],
        help="Explicit runtime-guarded symbol glob to allowlist (default: none).",
    )
    parser.add_argument(
        "--require-code",
        action="store_true",
        help="Fail if zero instructions are disassembled from an input file.",
    )
    parser.add_argument(
        "--objdump",
        default="objdump",
        help="Path to objdump executable (default: objdump).",
    )
    args = parser.parse_args(argv)

    if args.raw and args.raw_16:
        print("error: --raw and --raw-16 are mutually exclusive", file=sys.stderr)
        return 2

    all_violations: List[PurityViolation] = []
    total_insns = 0
    total_skipped = 0

    for fstr in args.files:
        fpath = Path(fstr)
        if not fpath.exists():
            print(f"error: file not found: {fpath}", file=sys.stderr)
            return 2
        try:
            disasm = disassemble(
                fpath, raw=args.raw, raw_16=args.raw_16, objdump=args.objdump
            )
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 2

        violations, insn_count, skipped_count = scan_disassembly(
            disasm,
            path_label=str(fpath),
            allow_sections=args.allow_section,
            allow_symbols=args.allow_symbol,
        )
        total_insns += insn_count
        total_skipped += skipped_count
        all_violations.extend(violations)

        if args.require_code and insn_count == 0:
            print(
                f"FAIL: {fpath} contained 0 disassembled instructions (--require-code).",
                file=sys.stderr,
            )
            return 1

    if all_violations:
        for v in all_violations:
            print(v.format(), file=sys.stderr)
        print(
            f"FAIL: 386 purity check found {len(all_violations)} forbidden "
            f"instruction(s) out of {total_insns} scanned.",
            file=sys.stderr,
        )
        return 1

    skip_note = f", {total_skipped} allowlisted" if total_skipped else ""
    print(
        f"PASS: 386 purity clean "
        f"({total_insns} instructions scanned across {len(args.files)} file(s){skip_note})."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
