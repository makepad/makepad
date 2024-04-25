(module
    (func $fib (export "fib") (param $n i64) (result i64)
        (if
            (i64.le_s (local.get $n) (i64.const 1))
            (then (return (local.get $n)))
        )
        (return
            (i64.add
                (call $fib
                  (i64.sub (local.get $n) (i64.const 1))
                )
                (call $fib
                  (i64.sub (local.get $n) (i64.const 2))
                )
            )
        )
    )
)
