/* Linker script for Rustux C programs (static, not position-independent) */

ENTRY(_start)

SECTIONS {
    . = 0x100000;
    __image_start = .;

    /* Code and read-only data */
    .text : {
        *(.text*)
        *(.rodata*)
    }

    /* Read-write data */
    . = ALIGN(4096);
    .data : {
        *(.data*)
    }
    .bss : {
        *(.bss*)
        *(COMMON)
    }
    __image_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment)
    }
}
