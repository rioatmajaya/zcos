/* Linker layout for ZC OS userspace tasks.
 *
 * Each task is an ET_EXEC linked at the user base 0x400000. The kernel maps
 * every PT_LOAD segment with user permissions and enters the ELF entry
 * point in ring 3 with a fresh stack; the task never returns.
 */

OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS {
    . = 0x400000;

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
