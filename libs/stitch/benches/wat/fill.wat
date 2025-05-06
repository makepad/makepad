(module
    (memory (export "memory") 16)
    (func (export "fill") (param $idx i32) (param $val i32) (param $count i32) 
        (block $break
            (loop $loop
                (br_if
                    $break
                    (i32.eqz
                        (local.get $count)
                    )
                )
                (i32.store8 offset=0
                    (local.get $idx)
                    (local.get $val)
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
        (return)
    )
)