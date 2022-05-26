(module
  ;; Store the ABI at global address 1024
  (data (i32.const 1024) "#\03\00\00\00\11\06\00\00\00Struct\02\00\00\00\0c\04\00\00\00Test\11\04\00\00\00Unit\00\00\00\000\10\00\00\00\000\10\00\00\00\00")

  ;; Some recursive function
  (func $f (param $0 i32) (result i32)
    (if
      (i32.lt_s
        (local.get $0)
        (i32.const 2)
      )
      (return
        (i32.const 1)
      )
    )
    (return
      (i32.add
        (call $f
          (i32.sub
            (local.get $0)
            (i32.const 1)
          )
        )
        (local.get $0)
      )
    )
  )

  ;; Simple main function that always returns `()`
  (func $Test_main (param $0 i32) (result i32)
    ;; Recursion starts!
    (drop
      (call $f
        (i32.sub
          (i32.const ${n})
          (i32.const 1)
        )
      )
    )
    ;; Recursion ends!

    (local.set 
      $0
      (call $scrypto_alloc
        (i32.const 1)
      )
    )
    (i32.add
      (local.get $0)
      (i32.const 4)
    )
    (i32.const 0)
    (i32.store8)
    (local.get $0)
  )

  (memory $0 1)
  (export "memory" (memory $0))
  (export "scrypto_alloc" (func $scrypto_alloc))
  (export "scrypto_free" (func $scrypto_free))
  (export "Test_main" (func $Test_main))
  (export "f" (func $f))

  ${memcpy}
  ${buffer}
)