;;! target = "x86_64"
;;! test = "winch"

(module
    (func (result i64)
        (local $foo i64)

        (i64.const 2)
        (local.set $foo)

        (local.get $foo)
        (i64.ctz)
    )
)
;; wasm[0]::function[0]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    8(%rdi), %r11
;;       movq    (%r11), %r11
;;       addq    $0x18, %r11
;;       cmpq    %rsp, %r11
;;       ja      0x5e
;;   1b: movq    %rdi, %r14
;;       subq    $0x18, %rsp
;;       movq    %rdi, 0x10(%rsp)
;;       movq    %rsi, 8(%rsp)
;;       movq    $0, (%rsp)
;;       movq    $2, %rax
;;       movq    %rax, (%rsp)
;;       movq    (%rsp), %rax
;;       bsfq    %rax, %rax
;;       movl    $0, %r11d
;;       sete    %r11b
;;       shlq    $6, %r11
;;       addq    %r11, %rax
;;       addq    $0x18, %rsp
;;       popq    %rbp
;;       retq
;;   5e: ud2
