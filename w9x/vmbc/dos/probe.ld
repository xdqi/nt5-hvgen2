/* Flat binary at offset 1000h of the COM segment (see stub.asm); bss goes
   into the file because DOS does not clear memory past the image. */
OUTPUT_FORMAT("elf32-i386")
ENTRY(probe_main)
SECTIONS
{
    . = 0x1000;
    .text : { *(.text.entry) *(.text*) }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) *(.bss*) *(COMMON) LONG(0) }
    _end = .;
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
