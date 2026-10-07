; Master boot record of `hvkit w98-disk`'s disks: load the first sector of the active partition
; with the INT 13h extensions (LBA) and jump to it, DL = the boot drive and DS:SI = its partition
; table entry, as DOS's MBR does. Windows 98's FAT16 boot sector needs no more.
; nasm -f bin -o mbr.bin mbr.asm; the bytes are in tools/hvkit/crates/media/src/w98_disk.rs.
        bits 16
        org 0x600

start:  cli
        xor ax, ax
        mov ss, ax
        mov sp, 0x7c00
        mov ds, ax
        mov es, ax
        sti
        cld
        mov si, 0x7c00                  ; move out of the way of the boot sector
        mov di, 0x600
        mov cx, 256
        rep movsw
        jmp 0:found                     ; continue in the copy
found:  mov si, table
        mov cx, 4
.next:  test byte [si], 0x80
        jnz .boot
        add si, 16
        loop .next
        mov si, noactive
        jmp fail
.boot:  mov eax, [si + 8]               ; first sector of the partition
        mov [dap.lba], eax
        push si
        mov si, dap
        mov ah, 0x42
        int 0x13
        pop si
        jc .err
        cmp word [0x7dfe], 0xaa55
        jne .err
        jmp 0:0x7c00
.err:   mov si, readerr
fail:   lodsb
        test al, al
        jz .halt
        mov ah, 0x0e
        mov bx, 7
        int 0x10
        jmp fail
.halt:  int 0x18                        ; no OS: let the BIOS try the next device
        hlt
        jmp .halt

dap:    db 16, 0
        dw 1                            ; one sector
        dw 0x7c00, 0                    ; to 0000:7C00
.lba:   dd 0, 0
noactive: db "No active partition", 13, 10, 0
readerr:  db "Error reading the boot sector", 13, 10, 0

        times 0x1be - ($ - $$) db 0
table:
