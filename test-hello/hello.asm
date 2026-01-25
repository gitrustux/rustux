; Minimal userspace test program
; This is a tiny program that tests syscall functionality
; Expected output: "Hello from userspace!" on debug console

BITS 64

section .text
global _start

_start:
    ; Write message to debug console (port 0xE9)
    ; This is NOT a syscall - direct port I/O for testing
    lea rsi, [rel msg]
    mov rcx, msg_len

.write_loop:
    test rcx, rcx
    jz .exit
    mov dx, 0xE9        ; Debug console port
    mov al, [rsi]
    out dx, al
    inc rsi
    dec rcx
    jmp .write_loop

.exit:
    ; Spin forever (no syscall support yet)
    cli                 ; Disable interrupts
.halt:
    hlt
    jmp .halt

section .rodata
msg: db "Hello from userspace!", 10
msg_len equ $ - msg
