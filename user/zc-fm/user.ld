/* Linker layout for the ZC OS file-manager client, linked at 0x4A0000 so its
 * image stays clear of the compositor and the other bring-up tasks. The kernel
 * maps it at the same virtual base it maps the terminal client at, because only
 * one task's image is live at that address at a time. */

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
