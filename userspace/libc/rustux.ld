/* Linker script for Rustux programs (static, not position-independent).
   The ELF headers are loaded with the code (at 0x100000) so the library
   can find its own program headers: the thread-local storage template
   (PT_TLS) and dl_iterate_phdr. */

ENTRY(_start)

SECTIONS {
    __image_start = 0x100000;
    . = 0x100000 + SIZEOF_HEADERS;

    /* Code and read-only data */
    .text : {
        *(.text .text.*)
    }
    .rodata : {
        *(.rodata .rodata.*)
        *(.gcc_except_table .gcc_except_table.*)
    }

    /* Read-write data */
    . = ALIGN(4096);
    /* Thread-local storage template: copied for each thread */
    .tdata : {
        *(.tdata .tdata.*)
    }
    .tbss : {
        *(.tbss .tbss.*)
        *(.tcommon)
    }
    /* Constructors, run by the library before main() */
    .init_array : {
        PROVIDE_HIDDEN(__init_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
        KEEP(*(.init_array))
        PROVIDE_HIDDEN(__init_array_end = .);
    }
    .fini_array : {
        PROVIDE_HIDDEN(__fini_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*)))
        KEEP(*(.fini_array))
        PROVIDE_HIDDEN(__fini_array_end = .);
    }
    .data.rel.ro : {
        *(.data.rel.ro .data.rel.ro.*)
    }
    .got : {
        *(.got) *(.got.plt)
    }
    .data : {
        *(.data .data.*)
    }
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    __image_end = .;

    /* No unwinding tables (programs are built with panic=abort or C) */
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment)
    }
}
