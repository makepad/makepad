(module
    (func $fac_iter (export "fac_iter") (param $n i64) (result i64)
        (local $acc i64)
        (local.set $acc
            (i64.const 1)
        )
        
        (block $break
            (br_if $break
                (i64.lt_u
                    (local.get $n)
                    (i64.const 2)
                )
            )
            (loop $continue
                (local.set
                    $acc
                    (i64.mul
                        (local.get $acc) 
                        (local.get $n)
                    )
                )
                (local.set $n
                    (i64.sub
                        (local.get $n)
                        (i64.const 1)
                    )
                )
                (br_if $continue
                    (i64.gt_s
                        (local.get $n)
                        (i64.const 1)
                    )
                )
            )
        )
        (local.get $acc)
    )
)