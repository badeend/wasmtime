;;! target = "x86_64"
;;! test = "compile"
;;! flags = " -C cranelift-enable-heap-access-spectre-mitigation -W memory64 -O static-memory-maximum-size=0 -O static-memory-guard-size=4294967295 -O dynamic-memory-guard-size=4294967295"

;; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
;; !!! GENERATED BY 'make-load-store-tests.sh' DO NOT EDIT !!!
;; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!

(module
  (memory i64 1)

  (func (export "do_store") (param i64 i32)
    local.get 0
    local.get 1
    i32.store8 offset=0x1000)

  (func (export "do_load") (param i64) (result i32)
    local.get 0
    i32.load8_u offset=0x1000))

;; wasm[0]::function[0]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    0x58(%rdi), %rsi
;;       movq    0x50(%rdi), %rax
;;       xorq    %rdi, %rdi
;;       leaq    0x1000(%rax, %rdx), %r11
;;       cmpq    %rsi, %rdx
;;       cmovaq  %rdi, %r11
;;       movb    %cl, (%r11)
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       retq
;;
;; wasm[0]::function[1]:
;;       pushq   %rbp
;;       movq    %rsp, %rbp
;;       movq    0x58(%rdi), %rsi
;;       movq    0x50(%rdi), %rax
;;       xorq    %rdi, %rdi
;;       leaq    0x1000(%rax, %rdx), %r11
;;       cmpq    %rsi, %rdx
;;       cmovaq  %rdi, %r11
;;       movzbq  (%r11), %rax
;;       movq    %rbp, %rsp
;;       popq    %rbp
;;       retq
