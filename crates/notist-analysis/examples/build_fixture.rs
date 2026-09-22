fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/packages/mermaid/wasm");
    use notist_model::{Type, abi};
    let registry = serde_json::to_string(&abi::Registration {
        elements: Default::default(),
        functions: [(
            "echo".into(),
            abi::Function {
                export: "echo".into(),
                params: vec![abi::Parameter {
                    name: "source".into(),
                    ty: Type::String,
                    default: None,
                }],
                result: Type::String,
            },
        )]
        .into_iter()
        .collect(),
    })?;
    let escaped = registry
        .bytes()
        .map(|b| format!("\\{b:02x}"))
        .collect::<String>();
    let source = format!(
        r#"(module
      (memory (export "memory") 32)
      (data (i32.const 0) "{escaped}")
      (func (export "alloc") (param i32) (result i32) i32.const 8192)
      (func (export "notist_register") (param i32 i32) (result i64) i64.const {})
      (func (export "echo") (param i32 i32) (result i64)
        local.get 0 i32.const 1 i32.add i64.extend_i32_u i64.const 32 i64.shl
        local.get 1 i32.const 2 i32.sub i64.extend_i32_u i64.or))"#,
        registry.len()
    );
    std::fs::create_dir_all(&root)?;
    std::fs::write(root.join("semantic.wat"), &source)?;
    std::fs::write(root.join("semantic.wasm"), wat::parse_str(source)?)?;
    let package = notist_analysis::package::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo"),
    )?;
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor/fixtures/package.json"),
        serde_json::to_vec_pretty(&package.request())?,
    )?;
    Ok(())
}
