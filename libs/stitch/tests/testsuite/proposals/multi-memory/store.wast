;; Multiple memories

(module
  (memory $mem1 1)
  (memory $mem2 1)

  (func (export "load1") (param i32) (result i64)
    (i64.load $mem1 (local.get 0))
  )
  (func (export "load2") (param i32) (result i64)
    (i64.load $mem2 (local.get 0))
  )

  (func (export "store1") (param i32 i64)
    (i64.store $mem1 (local.get 0) (local.get 1))
  )
  (func (export "store2") (param i32 i64)
    (i64.store $mem2 (local.get 0) (local.get 1))
  )
)

(invoke "store1" (i32.const 0) (i64.const 1))
(invoke "store2" (i32.const 0) (i64.const 2))
(assert_return (invoke "load1" (i32.const 0)) (i64.const 1))
(assert_return (invoke "load2" (i32.const 0)) (i64.const 2))


(module $M1
  (memory (export "mem") 1)

  (func (export "load") (param i32) (result i64)
    (i64.load (local.get 0))
  )
  (func (export "store") (param i32 i64)
    (i64.store (local.get 0) (local.get 1))
  )
)
(register "M1")

(module $M2
  (memory (export "mem") 1)

  (func (export "load") (param i32) (result i64)
    (i64.load (local.get 0))
  )
  (func (export "store") (param i32 i64)
    (i64.store (local.get 0) (local.get 1))
  )
)
(register "M2")

(invoke $M1 "store" (i32.const 0) (i64.const 1))
(invoke $M2 "store" (i32.const 0) (i64.const 2))
(assert_return (invoke $M1 "load" (i32.const 0)) (i64.const 1))
(assert_return (invoke $M2 "load" (i32.const 0)) (i64.const 2))

(module
  (memory $mem1 (import "M1" "mem") 1)
  (memory $mem2 (import "M2" "mem") 1)

  (func (export "load1") (param i32) (result i64)
    (i64.load $mem1 (local.get 0))
  )
  (func (export "load2") (param i32) (result i64)
    (i64.load $mem2 (local.get 0))
  )

  (func (export "store1") (param i32 i64)
    (i64.store $mem1 (local.get 0) (local.get 1))
  )
  (func (export "store2") (param i32 i64)
    (i64.store $mem2 (local.get 0) (local.get 1))
  )
)

(invoke "store1" (i32.const 0) (i64.const 1))
(invoke "store2" (i32.const 0) (i64.const 2))
(assert_return (invoke "load1" (i32.const 0)) (i64.const 1))
(assert_return (invoke "load2" (i32.const 0)) (i64.const 2))


(module
  (memory (export "mem") 2)
)
(register "M")

(module
  (memory $mem1 (import "M" "mem") 2)
  (memory $mem2 3)

  (data (memory $mem1) (i32.const 20) "\01\02\03\04\05")
  (data (memory $mem2) (i32.const 50) "\0A\0B\0C\0D\0E")

  (func (export "read1") (param i32) (result i32)
    (i32.load8_u $mem1 (local.get 0))
  )
  (func (export "read2") (param i32) (result i32)
    (i32.load8_u $mem2 (local.get 0))
  )

  (func (export "copy-1-to-2")
    (local $i i32)
    (local.set $i (i32.const 20))
    (loop $cont
      (br_if 1 (i32.eq (local.get $i) (i32.const 23)))
      (i32.store8 $mem2 (local.get $i) (i32.load8_u $mem1 (local.get $i)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $cont)
    )
  )

  (func (export "copy-2-to-1")
    (local $i i32)
    (local.set $i (i32.const 50))
    (loop $cont
      (br_if 1 (i32.eq (local.get $i) (i32.const 54)))
      (i32.store8 $mem1 (local.get $i) (i32.load8_u $mem2 (local.get $i)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $cont)
    )
  )
)

(assert_return (invoke "read2" (i32.const 20)) (i32.const 0))
(assert_return (invoke "read2" (i32.const 21)) (i32.const 0))
(assert_return (invoke "read2" (i32.const 22)) (i32.const 0))
(assert_return (invoke "read2" (i32.const 23)) (i32.const 0))
(assert_return (invoke "read2" (i32.const 24)) (i32.const 0))
(invoke "copy-1-to-2")
(assert_return (invoke "read2" (i32.const 20)) (i32.const 1))
(assert_return (invoke "read2" (i32.const 21)) (i32.const 2))
(assert_return (invoke "read2" (i32.const 22)) (i32.const 3))
(assert_return (invoke "read2" (i32.const 23)) (i32.const 0))
(assert_return (invoke "read2" (i32.const 24)) (i32.const 0))

(assert_return (invoke "read1" (i32.const 50)) (i32.const 0))
(assert_return (invoke "read1" (i32.const 51)) (i32.const 0))
(assert_return (invoke "read1" (i32.const 52)) (i32.const 0))
(assert_return (invoke "read1" (i32.const 53)) (i32.const 0))
(assert_return (invoke "read1" (i32.const 54)) (i32.const 0))
(invoke "copy-2-to-1")
(assert_return (invoke "read1" (i32.const 50)) (i32.const 10))
(assert_return (invoke "read1" (i32.const 51)) (i32.const 11))
(assert_return (invoke "read1" (i32.const 52)) (i32.const 12))
(assert_return (invoke "read1" (i32.const 53)) (i32.const 13))
(assert_return (invoke "read1" (i32.const 54)) (i32.const 0))


;; Store operator as the argument of control constructs and instructions

(module
  (memory 1)

  (func (export "as-block-value")
    (block (i32.store (i32.const 0) (i32.const 1)))
  )
  (func (export "as-loop-value")
    (loop (i32.store (i32.const 0) (i32.const 1)))
  )

  (func (export "as-br-value")
    (block (br 0 (i32.store (i32.const 0) (i32.const 1))))
  )
  (func (export "as-br_if-value")
    (block
      (br_if 0 (i32.store (i32.const 0) (i32.const 1)) (i32.const 1))
    )
  )
  (func (export "as-br_if-value-cond")
    (block
      (br_if 0 (i32.const 6) (i32.store (i32.const 0) (i32.const 1)))
    )
  )
  (func (export "as-br_table-value")
    (block
      (br_table 0 (i32.store (i32.const 0) (i32.const 1)) (i32.const 1))
    )
  )

  (func (export "as-return-value")
    (return (i32.store (i32.const 0) (i32.const 1)))
  )

  (func (export "as-if-then")
    (if (i32.const 1) (then (i32.store (i32.const 0) (i32.const 1))))
  )
  (func (export "as-if-else")
    (if (i32.const 0) (then) (else (i32.store (i32.const 0) (i32.const 1))))
  )
)

(assert_return (invoke "as-block-value"))
(assert_return (invoke "as-loop-value"))

(assert_return (invoke "as-br-value"))
(assert_return (invoke "as-br_if-value"))
(assert_return (invoke "as-br_if-value-cond"))
(assert_return (invoke "as-br_table-value"))

(assert_return (invoke "as-return-value"))

(assert_return (invoke "as-if-then"))
(assert_return (invoke "as-if-else"))

(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (i32.store32 (local.get 0) (i32.const 0)))"
  )
  "unknown operator"
)
(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (i32.store64 (local.get 0) (i64.const 0)))"
  )
  "unknown operator"
)

(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (i64.store64 (local.get 0) (i64.const 0)))"
  )
  "unknown operator"
)

(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (f32.store32 (local.get 0) (f32.const 0)))"
  )
  "unknown operator"
)
(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (f32.store64 (local.get 0) (f64.const 0)))"
  )
  "unknown operator"
)

(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (f64.store32 (local.get 0) (f32.const 0)))"
  )
  "unknown operator"
)
(assert_malformed
  (module quote
    "(memory 1)"
    "(func (param i32) (f64.store64 (local.get 0) (f64.const 0)))"
  )
  "unknown operator"
)
;; store should have no retval

(assert_invalid
  (module (memory 1) (func (param i32) (result i32) (i32.store (i32.const 0) (i32.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i64) (result i64) (i64.store (i32.const 0) (i64.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param f32) (result f32) (f32.store (i32.const 0) (f32.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param f64) (result f64) (f64.store (i32.const 0) (f64.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i32) (result i32) (i32.store8 (i32.const 0) (i32.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i32) (result i32) (i32.store16 (i32.const 0) (i32.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i64) (result i64) (i64.store8 (i32.const 0) (i64.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i64) (result i64) (i64.store16 (i32.const 0) (i64.const 1))))
  "type mismatch"
)
(assert_invalid
  (module (memory 1) (func (param i64) (result i64) (i64.store32 (i32.const 0) (i64.const 1))))
  "type mismatch"
)


(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty
      (i32.store)
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty
     (i32.const 0) (i32.store)
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-block
      (i32.const 0) (i32.const 0)
      (block (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-block
      (i32.const 0)
      (block (i32.const 0) (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-loop
      (i32.const 0) (i32.const 0)
      (loop (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-loop
      (i32.const 0)
      (loop (i32.const 0) (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-then
      (i32.const 0) (i32.const 0)
      (if (then (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-then
      (i32.const 0)
      (if (then (i32.const 0) (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-else
      (i32.const 0) (i32.const 0)
      (if (result i32) (then (i32.const 0)) (else (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-else
      (i32.const 0)
      (if (result i32) (then (i32.const 0)) (else (i32.const 0) (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-br
      (i32.const 0) (i32.const 0)
      (block (br 0 (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-br
      (i32.const 0)
      (block (br 0 (i32.const 0) (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-br_if
      (i32.const 0) (i32.const 0)
      (block (br_if 0 (i32.store) (i32.const 1)) )
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-br_if
      (i32.const 0)
      (block (br_if 0 (i32.const 0) (i32.store) (i32.const 1)) )
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-br_table
      (i32.const 0) (i32.const 0)
      (block (br_table 0 (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-br_table
      (i32.const 0)
      (block (br_table 0 (i32.const 0) (i32.store)))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-return
      (return (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-return
      (return (i32.const 0) (i32.store))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-select
      (select (i32.store) (i32.const 1) (i32.const 2))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-select
      (select (i32.const 0) (i32.store) (i32.const 1) (i32.const 2))
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty-in-call
      (call 1 (i32.store))
    )
    (func (param i32) (result i32) (local.get 0))
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $type-value-empty-in-call
      (call 1 (i32.const 0) (i32.store))
    )
    (func (param i32) (result i32) (local.get 0))
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $f (param i32) (result i32) (local.get 0))
    (type $sig (func (param i32) (result i32)))
    (table funcref (elem $f))
    (func $type-address-empty-in-call_indirect
      (block (result i32)
        (call_indirect (type $sig)
          (i32.store) (i32.const 0)
        )
      )
    )
  )
  "type mismatch"
)
(assert_invalid
  (module
    (memory 1)
    (func $f (param i32) (result i32) (local.get 0))
    (type $sig (func (param i32) (result i32)))
    (table funcref (elem $f))
    (func $type-value-empty-in-call_indirect
      (block (result i32)
        (call_indirect (type $sig)
          (i32.const 0) (i32.store) (i32.const 0)
        )
      )
    )
  )
  "type mismatch"
)


;; Type check

(assert_invalid (module (memory 1) (func (i32.store (f32.const 0) (i32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i32.store8 (f32.const 0) (i32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i32.store16 (f32.const 0) (i32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store (f32.const 0) (i32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store8 (f32.const 0) (i64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store16 (f32.const 0) (i64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store32 (f32.const 0) (i64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (f32.store (f32.const 0) (f32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (f64.store (f32.const 0) (f64.const 0)))) "type mismatch")

(assert_invalid (module (memory 1) (func (i32.store (i32.const 0) (f32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i32.store8 (i32.const 0) (f32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i32.store16 (i32.const 0) (f32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store (i32.const 0) (f32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store8 (i32.const 0) (f64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store16 (i32.const 0) (f64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (i64.store32 (i32.const 0) (f64.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (f32.store (i32.const 0) (i32.const 0)))) "type mismatch")
(assert_invalid (module (memory 1) (func (f64.store (i32.const 0) (i64.const 0)))) "type mismatch")
