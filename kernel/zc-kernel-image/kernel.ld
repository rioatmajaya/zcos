/* Linker layout for the freestanding ZC OS kernel image.
 *
 * The image is linked at its final higher-half virtual address. The loader
 * copies each PT_LOAD segment into a 2 MiB-aligned physical frame and maps
 * that frame to this virtual base with a single 2 MiB huge page, so every
 * symbol here is reachable the moment the loader switches CR3. */

OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS {
    . = 0xFFFFFFFF80000000;

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
