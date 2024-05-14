(module
    (func $fib_rec (export "fib_rec") (param $n i64) (result i64)
        (if
            (i64.lt_u
                (local.get $n)
                (i64.const 2)
            )
            (then
                (return
                    (local.get $n)
                )
            )
        )
        (return
            (i64.add
                (call $fib_rec
                    (i64.sub
                        (local.get $n)
                        (i64.const 2)
                    )
                )
                (call $fib_rec
                    (i64.sub
                        (local.get $n)
                        (i64.const 1)
                    )
                )
            )
        )
    )
)
