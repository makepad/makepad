(module
    (memory (export "memory") 16)
    (func (export "sum") (param $idx i32) (param $count i32) (result i64)
        (local $sum i64)
        (block $exit
            (loop $loop
                (br_if
                    $exit
                    (i32.eqz
                        (local.get $count)
                    )
                )
                (local.set $sum
                    (i64.add
                        (local.get $sum)
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
                (br $loop)
            )
        )
        (return
            (local.get $sum)
        )
    )
)