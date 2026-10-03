/* Linker layout for the ZC OS window client, linked at 0x4A0000 so its image
 * stays clear of the compositor and the other bring-up tasks. */

OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS {
    . = 0x4A0000;

    .text : {
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    .bss (NOLOAD) : {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
    }
}
