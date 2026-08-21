(module
  (type (;0;) (func (param i32 i32 i32)))
  (type (;1;) (func (param i32 i32) (result i32)))
  (type (;2;) (func (param i32)))
  (type (;3;) (func (param i32 i32 i32 i32) (result i32)))
  (type (;4;) (func))
  (import "cm32p2|notist:plugin/host" "call" (func $host_call (type 0)))
  (memory (export "cm32p2_memory") 16)
  (global $bump (mut i32) (i32.const 8192))
  (data (i32.const 1024) "{\"name\":\"core::text\",\"arguments\":[{\"name\":\"text\",\"value\":{\"type\":\"string\",\"value\":\"hello from host.call\"}}],\"body\":null}")
  (func $realloc (export "cm32p2_realloc") (type 3) (param $old i32) (param $old_size i32) (param $align i32) (param $new_size i32) (result i32)
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
(func $initialize (export "cm32p2_initialize") (type 4))
  (func $post (export "cm32p2||evaluate_post") (type 2) (param i32))
  (func (export "cm32p2||evaluate") (type 1) (param $ptr i32) (param $len i32) (result i32)
    (i32.const 1024) (i32.const 120) (i32.const 4096)
    call $host_call
    (i32.const 4096)))
