(module
      (memory (export "memory") 32)
      (data (i32.const 0) "\7b\22\66\75\6e\63\74\69\6f\6e\73\22\3a\7b\22\65\63\68\6f\22\3a\7b\22\65\78\70\6f\72\74\22\3a\22\65\63\68\6f\22\2c\22\70\61\72\61\6d\73\22\3a\5b\7b\22\6e\61\6d\65\22\3a\22\73\6f\75\72\63\65\22\2c\22\74\79\22\3a\7b\22\6b\69\6e\64\22\3a\22\73\74\72\69\6e\67\22\7d\7d\5d\2c\22\72\65\73\75\6c\74\22\3a\7b\22\6b\69\6e\64\22\3a\22\73\74\72\69\6e\67\22\7d\7d\7d\2c\22\65\6c\65\6d\65\6e\74\73\22\3a\7b\7d\7d")
      (func (export "alloc") (param i32) (result i32) i32.const 8192)
      (func (export "notist_register") (param i32 i32) (result i64) i64.const 133)
      (func (export "echo") (param i32 i32) (result i64)
        local.get 0 i32.const 1 i32.add i64.extend_i32_u i64.const 32 i64.shl
        local.get 1 i32.const 2 i32.sub i64.extend_i32_u i64.or))