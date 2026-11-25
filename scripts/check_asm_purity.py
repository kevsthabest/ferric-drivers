#!/usr/bin/env python3
"""386 purity check for hand-written NASM sources.

The binary checker (check_386_purity.py) sweeps objdump's linear
disassembly, which is sound for the Rust objects: their .text holds
only code, constants live in .rodata, and symbols mark every
function. It is NOT sound for the linked FERRIC.DLL: the Jemm hook
ABI requires a data dword immediately before each hook proc (the
cell Jemm writes the displaced handler address into), so the DLL's
single code section contains data, and a linear sweep manufactures
phantom instructions from whatever bytes it desyncs on — on
2026-11-21 it reported an `fdivl` whose bytes are the middle of a
`call` rel32, downstream of the _int33_hookproc header dword.

The DLL's content is exactly two inputs, so check the inputs:
the Rust object is swept by check_386_purity.py before conversion,
and the NASM sources are checked here, exactly, token by token,
against the same forbidden sets. A mnemonic in the source is the
instruction the assembler emits; there is no decoding to desync.
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_386_purity import FORBIDDEN_EXACT, FORBIDDEN_PREFIXES

SETCC_RE = re.compile(r"^set[a-z]{1,2}$")
LABEL_RE = re.compile(r"^[A-Za-z_.$][\w.$]*:\s*")
DIRECTIVES = {"bits", "cpu", "section", "global", "extern", "org",
              "align", "times"}


def scan(path: Path) -> list[str]:
    bad = []
    for lineno, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.split(";", 1)[0].strip()
        if not line or line.startswith("%"):
            continue
        line = LABEL_RE.sub("", line).strip()
        if not line:
            continue
        mnemonic = line.split()[0].lower().rstrip(":")
        if mnemonic in DIRECTIVES or mnemonic.startswith("."):
            continue
        if (mnemonic in FORBIDDEN_EXACT
                or SETCC_RE.match(mnemonic)
                or any(mnemonic.startswith(p) for p in FORBIDDEN_PREFIXES)):
            bad.append(f"{path}:{lineno}: forbidden mnemonic '{mnemonic}'")
    return bad


def main(argv: list[str]) -> int:
    violations: list[str] = []
    for name in argv[1:]:
        violations.extend(scan(Path(name)))
    if violations:
        for v in violations:
            print(v)
        print(f"FAIL: asm purity check found {len(violations)} forbidden "
              f"instruction(s).")
        return 1
    print(f"PASS: asm purity clean ({len(argv) - 1} source file(s)).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
