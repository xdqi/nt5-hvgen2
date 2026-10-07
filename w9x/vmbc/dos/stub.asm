; vmbcprob.com - DOS harness for vmbc: runs probe.c (linked at offset 1000h) in 32-bit protected mode with
; segment base = CS*16 and 4 GiB limits (no paging: physical = offset + base), then back in real mode checks
; that the BIOS still serves INT 13h (storvsc) and INT 16h (synthetic keyboard). Output goes to COM1.
; nasm -f bin -o vmbcprob.com stub.asm   (probe.bin next to it)
        cpu 686
        org 100h
        bits 16

start:
        mov ax, cs
        mov ds, ax
        mov es, ax
        movzx eax, ax
        shl eax, 4
        mov [base], eax
        mov ebx, eax
        mov si, gdt + 8
        mov cx, 4
.patch: mov [si + 2], bx
        mov edx, ebx
        shr edx, 16
        mov [si + 4], dl
        mov [si + 7], dh
        add si, 8
        loop .patch
        lea edx, [eax + gdt]
        mov [gdtr + 2], edx
        mov [rm_ptr + 2], cs
        cli
        mov [sp_save], sp
        lgdt [gdtr]
        mov eax, cr0
        or al, 1
        mov cr0, eax
        jmp dword 08h:pm_entry

        bits 32
pm_entry:
        mov ax, 10h
        mov ds, ax
        mov es, ax
        mov fs, ax
        mov gs, ax
        mov ss, ax
        mov esp, 0fff0h
        push dword [base]
        call c_blob
        add esp, 4
        mov [result], eax
        jmp word 18h:pm16

        bits 16
pm16:
        mov ax, 20h
        mov ds, ax
        mov es, ax
        mov fs, ax
        mov gs, ax
        mov ss, ax
        mov eax, cr0
        and al, 0feh
        mov cr0, eax
        jmp far [cs:rm_ptr]
rm_entry:
        mov ax, cs
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov sp, [sp_save]
        sti
        mov si, m_result
        call rm_serial
        mov eax, [result]
        call rm_hex8
        call rm_nl

        ; is the BIOS disk still alive?
        mov ax, 0201h
        mov cx, 0001h
        mov dx, 0080h
        mov bx, sectbuf
        int 13h
        mov si, m_int13ok
        jnc .s13
        mov si, m_int13bad
.s13:   call rm_serial

        ; and the BIOS keyboard? Wait up to 20 s (BDA ticks) for keys.
        mov si, m_keywait
        call rm_serial
        push 40h
        pop fs
        mov ebx, [fs:6ch]
        add ebx, 364
        xor di, di                      ; keys seen
.kw:    mov ah, 01h
        int 16h
        jz .nokey
        mov ah, 00h
        int 16h
        push ax
        mov si, m_key
        call rm_serial
        pop ax
        call rm_hex4
        call rm_nl
        inc di
        cmp di, 4
        jae .kdone
.nokey: cmp [fs:6ch], ebx
        jb .kw
.kdone: mov si, m_keys
        call rm_serial
        mov ax, di
        call rm_hex4
        call rm_nl
        mov ax, 4c00h
        int 21h

rm_serial:                              ; si = zero-terminated, to COM1
        lodsb
        test al, al
        jz .done
        call rm_putc
        jmp rm_serial
.done:  ret
rm_putc:
        push dx
        push ax
        mov dx, 3fdh
.w:     in al, dx
        test al, 20h
        jz .w
        pop ax
        mov dx, 3f8h
        out dx, al
        pop dx
        ret
rm_hex4:                                ; ax
        push cx
        mov cx, 4
.l:     rol ax, 4
        push ax
        and al, 15
        add al, '0'
        cmp al, '9'
        jbe .p
        add al, 7
.p:     call rm_putc
        pop ax
        loop .l
        pop cx
        ret
rm_hex8:                                ; eax
        push eax
        shr eax, 16
        call rm_hex4
        pop eax
        jmp rm_hex4
rm_nl:  mov al, 13
        call rm_putc
        mov al, 10
        jmp rm_putc

m_result  db 'vmbc: probe returned ', 0
m_int13ok db 'vmbc: INT 13h ok', 13, 10, 0
m_int13bad db 'vmbc: INT 13h FAILED', 13, 10, 0
m_keywait db 'vmbc: KEYWAIT', 13, 10, 0
m_key     db 'vmbc: INT 16h key ', 0
m_keys    db 'vmbc: keys seen ', 0

gdt:    dq 0
        dw 0ffffh, 0
        db 0, 9ah, 0cfh, 0              ; 08h code32, base patched, 4 GiB
        dw 0ffffh, 0
        db 0, 92h, 0cfh, 0              ; 10h data32
        dw 0ffffh, 0
        db 0, 9ah, 0, 0                 ; 18h code16
        dw 0ffffh, 0
        db 0, 92h, 0, 0                 ; 20h data16
gdt_end:
gdtr:   dw gdt_end - gdt - 1
        dd 0
rm_ptr: dw rm_entry
        dw 0
sp_save: dw 0
base:   dd 0
result: dd 0
sectbuf: times 512 db 0

        times 1000h - 100h - ($ - $$) db 0
c_blob: incbin "probe.bin"
