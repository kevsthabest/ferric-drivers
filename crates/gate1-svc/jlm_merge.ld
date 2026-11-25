/* Partial-link script to merge Rust's per-function ELF32 sections into
 * standard .text / .rodata / .data / .bss sections for OpenWatcom wlink
 * PE32 JLM linking (out/FERRIC.DLL).
 */
SECTIONS {
    .text 0 : ALIGN(4) {
        *(.text.entry)
        *(.text .text.*)
    }
    .rodata 0 : ALIGN(4) {
        *(.rodata .rodata.*)
    }
    .data 0 : ALIGN(4) {
        *(.data .data.*)
    }
    .bss 0 : ALIGN(4) {
        *(.bss .bss.*)
        *(COMMON)
    }
    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
        *(.note*)
        *(.debug*)
        *(.llvmbc*)
        *(.llvmcmd*)
    }
}
