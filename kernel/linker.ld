/* Kernel image layout. Limine maps PT_LOAD segments with their ELF permissions;
   arch::paging then rebuilds the same mappings from the __*_start/end symbols. */
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

PHDRS
{
    rodata PT_LOAD FLAGS(4);    /* R */
    text   PT_LOAD FLAGS(5);    /* R X */
    data   PT_LOAD FLAGS(6);    /* R W */
}

SECTIONS
{
    . = 0xffffffff80000000;
    __kernel_start = .;

    __rodata_start = .;
    /* Written by the bootloader before entry, read-only afterwards. */
    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :rodata
    .rodata : { *(.rodata .rodata.*) } :rodata
    . = ALIGN(4096);
    __rodata_end = .;

    __text_start = .;
    .text : { *(.text .text.*) } :text
    . = ALIGN(4096);
    __text_end = .;

    __data_start = .;
    .data : { *(.data .data.*) } :data
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data
    . = ALIGN(4096);
    __data_end = .;

    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) }
}
