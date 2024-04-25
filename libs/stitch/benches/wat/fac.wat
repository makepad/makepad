(module
    (func $fac (export "fac") (param $n i64) (result i64)
        (if (result i64)
            (i64.eq (local.get $n) (i64.const 0))
            (then (i64.const 1))
            (else
                (i64.mul
                    (local.get $n)
                    (call $fac
                        (i64.sub
                            (local.get $n)
                            (i64.const 1)
                        )
                    )
                )
            )
        )
    )
)
