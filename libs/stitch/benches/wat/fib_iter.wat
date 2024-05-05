(func $fib_iter (export "fib_iter") (param $n i64) (result i64)
    (local $n1 i64)
    (local $n2 i64)
    (local $i i64)
    (local $tmp i64)

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
    (local.set $n1
        (i64.const 0)
    )
    (local.set $n2
        (i64.const 1)
    )
    (local.set $i
        (i64.const 2)
    )
    (block $break
        (loop $continue
            (local.set $tmp
                (i64.add
                    (local.get $n1)
                    (local.get $n2)
                )
            )
            (local.set $n1
                (local.get $n2)
            )
            (local.set $n2
                (local.get $tmp)
            )
            (br_if $break
                (i64.eq
                    (local.get $i)
                    (local.get $n)
                )
            )
            (local.set $i
                (i64.add
                    (local.get $i)
                    (i64.const 1)
                )
            )
            (br $continue)
        )
    )
    (local.get $n2)
)