(module
    (memory (export "memory") 16)
    (func (export "sum") (param $idx i32) (param $count i32) (result i64)
        (local $acc i64)

        (block $break
            (loop $continue
                (br_if
                    $break
                    (i32.eqz
                        (local.get $count)
                    )
                )
                (local.set $acc
                    (i64.add
                        (local.get $acc)
                        (i64.load8_u offset=0
                            (local.get $idx)
                        )
                    )
                )
                (local.set $idx
                    (i32.add
                        (local.get $idx)
                        (i32.const 1)
                    )
                )
                (local.set $count
                    (i32.sub
                        (local.get $count)
                        (i32.const 1)
                    )
                )
                (br $continue)
            )
        )
        (return
            (local.get $acc)
        )
    )
)