(module
    (memory (export "memory") 16)
    (func (export "memory_fill") (param $val i32) (param $count i32) 
        (block $exit
            (loop $loop
                (br_if
                    $exit
                    (i32.eqz
                        (local.get $count)
                    )
                )
                (local.set $count
                    (i32.sub
                        (local.get $count)
                        (i32.const 1)
                    )
                )
                (i32.store8 offset=0
                    (local.get $count)
                    (local.get $val)
                )
                (br $loop)
            )
        )
        (return)
    )
)