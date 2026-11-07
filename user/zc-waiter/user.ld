/* Linker layout for the ZC OS event-wait proof task.
 *
 * A runtime spawn builds a private address space, so this base is free even
 * though it sits above the boot images. Keeping it distinct makes a symbol in
 * a crash report name the proof image it came from. */

OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS {
    . = 0x4B0000;

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
