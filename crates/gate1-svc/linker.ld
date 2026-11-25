/* Linker script for 32-bit freestanding Gate 1 resident Rust service
 * (i386-unknown-none). Linked at segment-relative VMA 0x1800 inside the
 * 64 KiB DOS .COM image, matching CODE32_SEL / DATA32_SEL (base = CS << 4)
 * in crates/gate1-svc/asm/svctest.asm.
 *
 * Fixed .bss placement at 0x6800..0x77FF allows svctest.asm to zero-init
 * .bss deterministically before invoking gate1_svc_entry.
 */
ENTRY(gate1_svc_entry)

SECTIONS {
    . = 0x1800;

    .text : ALIGN(4) {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }

    .rodata : ALIGN(4) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4) {
        *(.data .data.*)
    }

    ASSERT(. <= 0x6800, "gate1_svc_core code+rodata+data exceeds 20 KiB slot (0x1800..0x67FF)")

    . = 0x6800;
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    ASSERT(. <= 0x7800, "gate1_svc_core .bss exceeds 4 KiB slot (0x6800..0x77FF)")

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
        *(.note*)
    }
}
