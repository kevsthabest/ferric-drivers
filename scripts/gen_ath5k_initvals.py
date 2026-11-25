#!/usr/bin/env python3
"""Extract exact AR2413 (RF2413) 802.11g initvals, 2.4 GHz RF gain table,
and RF bank register values from vendor/ref/linux-ath/ath5k/ (pin c44104f3)
using the C compiler inside ferric-dos-build:gate0 for 100% exact macro expansion.
"""

import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ATH5K = ROOT / "vendor" / "ref" / "linux-ath" / "ath5k"
OUT = ROOT / "crates" / "gate1-svc" / "src" / "ath5k_initvals.rs"


def extract_raw(path: Path, name: str) -> str:
    s = path.read_text(encoding="utf-8")
    m = re.search(
        rf"static\s+const\s+struct\s+\w+\s+{name}\[\]\s*=\s*\{{(.*?)\n\}};",
        s,
        re.S,
    )
    if not m:
        raise RuntimeError(f"table {name} not found in {path}")
    return m.group(0)


def main() -> int:
    iv = ATH5K / "initvals.c"
    rg = ATH5K / "rfgain.h"
    rb = ATH5K / "rfbuffer.h"

    blocks = [
        extract_raw(iv, "ar5212_ini_mode_start"),
        extract_raw(iv, "ar5212_ini_common_start"),
        extract_raw(iv, "rf2413_ini_mode_end"),
        extract_raw(iv, "rf2413_ini_common_end"),
        extract_raw(iv, "rf5112_ini_bbgain"),
        extract_raw(rg, "rfgain_2413"),
        extract_raw(rb, "rfb_2413"),
    ]

    c_src = (
        "#include <stdio.h>\n"
        "#include <stdint.h>\n"
        "typedef uint8_t u8;\n"
        "typedef uint16_t u16;\n"
        "typedef uint32_t u32;\n"
        "#define AR5K_INI_WRITE 0\n"
        "#define AR5K_INI_READ 1\n"
        "struct ath5k_ini { u16 ini_register; u32 ini_value; u8 ini_mode; };\n"
        "struct ath5k_ini_mode { u16 mode_register; u32 mode_value[3]; };\n"
        "struct ath5k_ini_rfgain { u16 rfg_register; u32 rfg_value[2]; };\n"
        "struct ath5k_ini_rfbuffer { u8 rfb_bank; u16 rfb_ctrl_register; u32 rfb_mode_data[3]; };\n"
        '#include "reg.h"\n'
        + "\n".join(blocks)
        + """
#define N(a) (sizeof(a) / sizeof((a)[0]))
static void dump_mode(const struct ath5k_ini_mode *a, size_t n) {
    for (size_t i = 0; i < n; i++)
        printf("    (0x%04X, 0x%08X),\\n", a[i].mode_register, a[i].mode_value[2]);
}
static void dump_ini(const struct ath5k_ini *a, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (a[i].ini_mode == AR5K_INI_WRITE)
            printf("    (0x%04X, 0x%08X),\\n", a[i].ini_register, a[i].ini_value);
    }
}
int main(void) {
    printf("//! Auto-generated AR2413 (RF2413) 802.11g initvals from vendor/ref/linux-ath/ath5k/ (pin c44104f3).\\n");
    printf("//! Regenerate with: python3 scripts/gen_ath5k_initvals.py\\n\\n");
    printf("pub const AR2413_11G_INITVALS: &[(u16, u32)] = &[\\n");
    dump_mode(ar5212_ini_mode_start, N(ar5212_ini_mode_start));
    dump_ini(ar5212_ini_common_start, N(ar5212_ini_common_start));
    dump_mode(rf2413_ini_mode_end, N(rf2413_ini_mode_end));
    dump_ini(rf2413_ini_common_end, N(rf2413_ini_common_end));
    dump_ini(rf5112_ini_bbgain, N(rf5112_ini_bbgain));
    printf("];\\n\\n");
    printf("pub const RF2413_2GHZ_RFGAIN: &[(u16, u32)] = &[\\n");
    for (size_t i = 0; i < N(rfgain_2413); i++)
        printf("    (0x%04X, 0x%08X),\\n", rfgain_2413[i].rfg_register, rfgain_2413[i].rfg_value[1]);
    printf("];\\n\\n");
    printf("pub const RF2413_11G_RFBANKS: &[(u16, u32)] = &[\\n");
    for (size_t i = 0; i < N(rfb_2413); i++)
        printf("    (0x%04X, 0x%08X),\\n", rfb_2413[i].rfb_ctrl_register, rfb_2413[i].rfb_mode_data[2]);
    printf("];\\n");
    return 0;
}
"""
    )

    with tempfile.TemporaryDirectory() as td:
        c_file = Path(td) / "gen.c"
        exe = Path(td) / "gen"
        c_file.write_text(c_src, encoding="utf-8")
        subprocess.run(
            ["gcc", "-O0", f"-I{ATH5K}", str(c_file), "-o", str(exe)],
            check=True,
        )
        rust_src = subprocess.check_output([str(exe)]).decode("utf-8")
    OUT.write_text(rust_src, encoding="utf-8", newline="\n")
    print(f"Generated {OUT} ({len(rust_src)} bytes, {len(rust_src.splitlines())} lines)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
