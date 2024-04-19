;; Syntactic types (validation time)

;; Simple types.

(module
  (type $t1 (func (param f32 f32) (result f32)))
  (type $t2 (func (param $x f32) (param $y f32) (result f32)))

  (func $f1 (param $r (ref $t1)) (call $f2 (local.get $r)))
  (func $f2 (param $r (ref $t2)) (call $f1 (local.get $r)))
)


;; Indirect types.

(module
  (type $s0 (func (param i32) (result f32)))
  (type $s1 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $s2 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $t1 (func (param (ref $s1)) (result (ref $s2))))
  (type $t2 (func (param (ref $s2)) (result (ref $s1))))

  (func $f1 (param $r (ref $t1)) (call $f2 (local.get $r)))
  (func $f2 (param $r (ref $t2)) (call $f1 (local.get $r)))
)


;; Recursive types.

(assert_invalid
  (module
    (type $t (func (result (ref $t))))
  )
  "unknown type"
)

(assert_invalid
  (module
    (type $t1 (func (param (ref $t2))))
    (type $t2 (func (param (ref $t1))))
  )
  "unknown type"
)


;; Semantic types (run time)

;; Simple types.

(module
  (type $t1 (func (param f32 f32)))
  (type $t2 (func (param $x f32) (param $y f32)))

  (func $f1 (type $t1))
  (func $f2 (type $t2))
  (table funcref (elem $f1 $f2))

  (func (export "run")
    (call_indirect (type $t1) (f32.const 1) (f32.const 2) (i32.const 1))
    (call_indirect (type $t2) (f32.const 1) (f32.const 2) (i32.const 0))
  )
)
(assert_return (invoke "run"))


;; Indirect types.

(module
  (type $s0 (func (param i32)))
  (type $s1 (func (param i32 (ref $s0))))
  (type $s2 (func (param i32 (ref $s0))))
  (type $t1 (func (param (ref $s1))))
  (type $t2 (func (param (ref $s2))))

  (func $s1 (type $s1))
  (func $s2 (type $s2))
  (func $f1 (type $t1))
  (func $f2 (type $t2))
  (table funcref (elem $f1 $f2 $s1 $s2))

  (func (export "run")
    (call_indirect (type $t1) (ref.func $s1) (i32.const 0))
    (call_indirect (type $t1) (ref.func $s1) (i32.const 1))
    (call_indirect (type $t1) (ref.func $s2) (i32.const 0))
    (call_indirect (type $t1) (ref.func $s2) (i32.const 1))
    (call_indirect (type $t2) (ref.func $s1) (i32.const 0))
    (call_indirect (type $t2) (ref.func $s1) (i32.const 1))
    (call_indirect (type $t2) (ref.func $s2) (i32.const 0))
    (call_indirect (type $t2) (ref.func $s2) (i32.const 1))
  )
)
(assert_return (invoke "run"))


;; Semantic types (link time)

;; Simple types.

(module
  (type $t1 (func (param f32 f32) (result f32)))
  (func (export "f") (param (ref $t1)))
)
(register "M")
(module
  (type $t2 (func (param $x f32) (param $y f32) (result f32)))
  (func (import "M" "f") (param (ref $t2)))
)


;; Indirect types.

(module
  (type $s0 (func (param i32) (result f32)))
  (type $s1 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $s2 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $t1 (func (param (ref $s1)) (result (ref $s2))))
  (type $t2 (func (param (ref $s2)) (result (ref $s1))))
  (func (export "f1") (param (ref $t1)))
  (func (export "f2") (param (ref $t1)))
)
(register "N")
(module
  (type $s0 (func (param i32) (result f32)))
  (type $s1 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $s2 (func (param i32 (ref $s0)) (result (ref $s0))))
  (type $t1 (func (param (ref $s1)) (result (ref $s2))))
  (type $t2 (func (param (ref $s2)) (result (ref $s1))))
  (func (import "N" "f1") (param (ref $t1)))
  (func (import "N" "f1") (param (ref $t2)))
  (func (import "N" "f2") (param (ref $t1)))
  (func (import "N" "f2") (param (ref $t1)))
)
