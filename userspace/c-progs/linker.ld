/* Linker script for Rustux C userspace programs */

ENTRY(_start)

SECTIONS {
    /* Program code and read-only data */
    /* Load at 1MB (standard x86_64 userspace load address) */
    . = 0x100000;

    .text : {
        *(.text*)
        *(.rodata*)
    }

    /* Read-write data (initialized) */
    .data : {
        *(.data*)
    }

    /* Read-write data (uninitialized) */
    .bss : {
        *(.bss*)
        *(COMMON)
    }

    /* Stack grows down from high memory */
    /* Reserve 1MB for stack at 8MB */
    . = 0x800000;
    .stack : {
        . = . + 0x100000;  /* 1MB stack */
    }

    /* Discard unwind sections */
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note.gnu.build-id)
    }
}
