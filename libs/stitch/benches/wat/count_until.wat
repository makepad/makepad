(module
  (func (export "count_until") (param $n i64) (result i64)
    (local $i i64)
    (loop
        (br_if
            0
            (i64.ne
                (local.tee $i
                    (i64.add
                        (local.get $i)
                        (i64.const 1)
                    )
                )
                (local.get $n)
            )
        )
    )
    (return
        (local.get $i)
    )
  )
)
