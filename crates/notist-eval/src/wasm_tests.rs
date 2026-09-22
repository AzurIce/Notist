use super::*;
use serde_json::{Value as Json, json};

fn location() -> Location {
    Location {
        source: "call.notc".into(),
        offset: 13,
    }
}

fn descriptor() -> abi::Registration {
    abi::Registration {
        elements: Default::default(),
        functions: [(
            "echo".into(),
            abi::Function {
                export: "echo".into(),
                params: vec![abi::Parameter {
                    name: "value".into(),
                    ty: Type::Optional(Box::new(Type::Int)),
                    default: Some(abi::Value::None),
                }],
                result: Type::Any,
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn module(registry: &Json, body: &str) -> Vec<u8> {
    let registry = registry.to_string();
    let escaped: String = registry.bytes().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 1)
        (data (i32.const 0) "{escaped}")
        (func (export "alloc") (param i32) (result i32) i32.const 8192)
        (func (export "notist_register") (param i32 i32) (result i64) i64.const {})
        {body})"#,
        registry.len()
    ))
    .unwrap()
}

const ECHO: &str = r#"(func (export "echo") (param i32 i32) (result i64)
    local.get 0 i32.const 1 i32.add i64.extend_i32_u i64.const 32 i64.shl
    local.get 1 i32.const 2 i32.sub i64.extend_i32_u i64.or)"#;

#[test]
fn typed_registration_preserves_explicit_none_default() {
    let bytes = module(&serde_json::to_value(descriptor()).unwrap(), ECHO);
    let (env, _) = registration(&bytes, "plugin.wasm", &location()).unwrap();
    let Value::External(f) = &env["echo"] else {
        panic!()
    };
    assert_eq!(f.params[0].ty, Type::Optional(Box::new(Type::Int)));
    assert!(matches!(f.defaults["value"], Value::None));
    assert_eq!(f.path, "plugin.wasm");
    let result = invoke(&bytes, "echo", &[Value::Int(7)], &Type::Int, &location()).unwrap();
    assert!(matches!(result, Value::Int(7)));
}

#[test]
fn invalid_registration_is_rejected_before_installation() {
    let good = serde_json::to_value(descriptor()).unwrap();
    let cases: &[(&str, Json, &str)] = &[
        (
            "/functions/echo/params/0/ty",
            json!("Int?"),
            "invalid registration encoding",
        ),
        (
            "/functions/echo/params/0/ty",
            json!({"kind":"unknown"}),
            "invalid registration encoding",
        ),
        (
            "/functions/echo/params/0/name",
            json!("let"),
            "invalid or duplicate parameter",
        ),
        (
            "/functions/echo/params/0/default",
            json!({"kind":"string", "value":"bad"}),
            "default type mismatch",
        ),
        (
            "/functions/echo/result",
            json!({"kind":"function"}),
            "cannot cross",
        ),
        (
            "/functions/echo/params/0/ty",
            json!({"kind":"optional", "inner":{"kind":"module"}}),
            "cannot cross",
        ),
        (
            "/functions/echo/export",
            json!("missing"),
            "missing function export",
        ),
    ];
    for (pointer, value, expected) in cases {
        let mut registry = good.clone();
        *registry.pointer_mut(pointer).unwrap() = value.clone();
        let err = registration(&module(&registry, ECHO), "p.wasm", &location()).unwrap_err();
        assert!(err.contains(expected), "{pointer}: {err}");
    }
    let mut duplicate = descriptor();
    let function = duplicate.functions.get_mut("echo").unwrap();
    function.params.push(function.params[0].clone());
    assert!(
        registration(
            &module(&serde_json::to_value(duplicate).unwrap(), ECHO),
            "p.wasm",
            &location()
        )
        .unwrap_err()
        .contains("duplicate parameter")
    );
    let mut invalid_name = descriptor();
    let f = invalid_name.functions.remove("echo").unwrap();
    invalid_name.functions.insert("wasm".into(), f);
    assert!(
        registration(
            &module(&serde_json::to_value(invalid_name).unwrap(), ECHO),
            "p.wasm",
            &location()
        )
        .unwrap_err()
        .contains("invalid registered name")
    );
    let bad_export = module(&good, r#"(func (export "echo"))"#);
    assert!(
        registration(&bad_export, "p.wasm", &location())
            .unwrap_err()
            .contains("invalid ABI")
    );
}

#[test]
fn invocation_enforces_types_and_transport_boundary() {
    let bytes = module(&serde_json::to_value(descriptor()).unwrap(), ECHO);
    assert!(
        invoke(
            &bytes,
            "echo",
            &[Value::Bool(true)],
            &Type::Int,
            &location()
        )
        .unwrap_err()
        .contains("declared result")
    );
    assert!(
        invoke(
            &bytes,
            "echo",
            &[Value::List(vec![Value::Named("f".into())])],
            &Type::Any,
            &location()
        )
        .unwrap_err()
        .contains("cannot cross")
    );
    let error = Value::Content(crate::Content::error(
        "bad",
        notist_model::DiagnosticCode::Evaluation,
        Location {
            source: "original".into(),
            offset: 0,
        },
    ));
    let result = invoke(&bytes, "echo", &[error], &Type::Int, &location()).unwrap();
    assert!(result.is_error());
    let Value::Content(actual) = result else {
        panic!()
    };
    assert_eq!(actual.location, location());
}

#[test]
fn malformed_output_and_runaway_plugins_are_bounded() {
    let registry = serde_json::to_value(descriptor()).unwrap();
    let malformed = module(
        &registry,
        r#"(func (export "echo") (param i32 i32) (result i64) i64.const 1)"#,
    );
    assert!(
        invoke(&malformed, "echo", &[], &Type::Any, &location())
            .unwrap_err()
            .contains("invalid result encoding")
    );
    let runaway = module(
        &registry,
        r#"(func (export "echo") (param i32 i32) (result i64) (loop $forever br $forever) i64.const 0)"#,
    );
    assert!(
        invoke(&runaway, "echo", &[], &Type::Any, &location())
            .unwrap_err()
            .contains("fuel")
    );
    let oversized = module(
        &registry,
        r#"(func (export "echo") (param i32 i32) (result i64) i64.const 1048577)"#,
    );
    assert!(
        invoke(&oversized, "echo", &[], &Type::Any, &location())
            .unwrap_err()
            .contains("result exceeds")
    );
    let out_of_bounds = module(
        &registry,
        r#"(func (export "echo") (param i32 i32) (result i64) i64.const -4294967295)"#,
    );
    assert!(invoke(&out_of_bounds, "echo", &[], &Type::Any, &location()).is_err());
}
