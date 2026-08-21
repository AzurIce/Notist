(module
  (memory (export "memory") 16)
  (global $bump (mut i32) (i32.const 4096))
  (func $realloc (export "cabi_realloc") (param $old i32) (param $old_size i32) (param $align i32) (param $new_size i32) (result i32)
    (local $ptr i32)
    (if (i32.eqz (local.get $new_size)) (then (return (local.get $align))))
    (if (i32.ne (local.get $old) (i32.const 0)) (then (return (local.get $old))))
    (global.get $bump)
    (i32.sub (local.get $align) (i32.const 1))
    i32.add
    (local.get $align)
    (i32.const -1)
    i32.xor
    i32.and
    local.set $ptr
    (local.get $ptr)
    (local.get $new_size)
    i32.add
    global.set $bump
    (local.get $ptr))
  (data (i32.const 1024) "[{\"type\":\"leaf\",\"name\":\"demo::echo\",\"fields\":[{\"name\":\"message\",\"value\":{\"type\":\"string\",\"value\":\"hello from component\"}}],\"body\":[],\"block\":true}]")
  (func (export "evaluate") (param $ptr i32) (param $len i32) (result i32)
    (local $res i32)
    (i32.const 4096)
    local.set $res
    (i32.store (local.get $res) (i32.const 0))
    (i32.store (i32.add (local.get $res) (i32.const 4)) (i32.const 1024))
    (i32.store (i32.add (local.get $res) (i32.const 8)) (i32.const 147))
    (local.get $res)))
