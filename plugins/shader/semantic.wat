(module
  (memory (export "memory") 1)

  ;; Binary response layout:
  ;;   byte 0: ok = 1
  ;;   bytes 1..5: i32 width (little endian)
  ;;   bytes 5..9: i32 height (little endian)
  (func (export "evaluate") (param $ptr i32) (param $len i32) (result i32)
    (local $source_len i32)
    (local $width_addr i32)

    ;; source_len = i32.load(ptr)
    local.get $ptr
    i32.load
    local.set $source_len

    ;; width_addr = ptr + 4 + source_len
    local.get $ptr
    i32.const 4
    i32.add
    local.get $source_len
    i32.add
    local.set $width_addr

    ;; write ok byte
    i32.const 8192
    i32.const 1
    i32.store8

    ;; write width
    i32.const 8193
    local.get $width_addr
    i32.load
    i32.store

    ;; write height
    i32.const 8197
    local.get $width_addr
    i32.const 4
    i32.add
    i32.load
    i32.store

    i32.const 8192
  )
)
