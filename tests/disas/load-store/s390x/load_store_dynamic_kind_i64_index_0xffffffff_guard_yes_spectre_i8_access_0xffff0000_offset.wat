;;! target = "s390x"
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
    i32.store8 offset=0xffff0000)

  (func (export "do_load") (param i64) (result i32)
    local.get 0
    i32.load8_u offset=0xffff0000))

;; wasm[0]::function[0]:
;;       stmg    %r7, %r15, 0x38(%r15)
;;       lgr     %r1, %r15
;;       aghi    %r15, -0xa0
;;       stg     %r1, 0(%r15)
;;       lg      %r3, 0x58(%r2)
;;       lghi    %r7, 0
;;       lgr     %r8, %r4
;;       ag      %r8, 0x50(%r2)
;;       llilh   %r2, 0xffff
;;       agrk    %r2, %r8, %r2
;;       clgr    %r4, %r3
;;       locgrh  %r2, %r7
;;       stc     %r5, 0(%r2)
;;       lmg     %r7, %r15, 0xd8(%r15)
;;       br      %r14
;;
;; wasm[0]::function[1]:
;;       stmg    %r7, %r15, 0x38(%r15)
;;       lgr     %r1, %r15
;;       aghi    %r15, -0xa0
;;       stg     %r1, 0(%r15)
;;       lg      %r3, 0x58(%r2)
;;       lghi    %r5, 0
;;       lgr     %r7, %r4
;;       ag      %r7, 0x50(%r2)
;;       llilh   %r2, 0xffff
;;       agrk    %r2, %r7, %r2
;;       clgr    %r4, %r3
;;       locgrh  %r2, %r5
;;       llc     %r2, 0(%r2)
;;       lmg     %r7, %r15, 0xd8(%r15)
;;       br      %r14
