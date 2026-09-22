use notist_ir::{Content, Env, Item, Location, Target, Value};
use notist_model::abi;

fn location(source: &str) -> Location {
    Location {
        source: source.into(),
        offset: 7,
    }
}

#[test]
fn nested_data_roundtrip_preserves_variants_and_attributes() {
    let original = Value::List(vec![
        Value::String("text".into()),
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::Bool(true),
        Value::None,
        Value::Dict(Env::from([("kind".into(), Value::String("item".into()))])),
        Value::Content(Item {
            span: None,
            origin: None,
            name: "custom".into(),
            args: Env::new(),
            attributes: Env::from([("count".into(), Value::Int(3))]),
            location: location("original"),
        }),
        Value::Content(Content::seq(vec![
            Content::text("body"),
            Content::link(
                Target::new("p0::doc", vec!["part".into()]),
                location("original"),
            ),
            Content::error(
                "error",
                notist_model::DiagnosticCode::Evaluation,
                location("original"),
            ),
        ])),
    ]);
    let encoded = original.to_abi().unwrap();
    let wire = serde_json::to_vec(&encoded).unwrap();
    let decoded = Value::from_abi(serde_json::from_slice(&wire).unwrap(), &location("call"));
    assert_eq!(decoded.to_abi().unwrap(), encoded);
    let Value::List(values) = decoded else {
        panic!()
    };
    let Value::Content(item) = &values[6] else {
        panic!()
    };
    assert_eq!(item.location, location("call"));
    let mut warnings = vec![];
    let Value::Content(content) = &values[7] else {
        panic!()
    };
    content.warnings(&mut warnings);
    assert_eq!(warnings, ["call:7: error"]);
}

#[test]
fn runtime_state_is_rejected_even_when_nested() {
    for value in [Value::Named("f".into()), Value::Module("p0::doc".into())] {
        assert!(value.to_abi().is_err());
        assert!(
            Value::Content(Item {
                span: None,
                origin: None,
                name: "custom".into(),
                args: Env::new(),
                attributes: Env::from([("hidden".into(), Value::List(vec![value]))]),
                location: location("source"),
            })
            .to_abi()
            .is_err()
        );
    }
}

#[test]
fn malformed_values_are_not_coerced() {
    for json in [
        r#"{"kind":"int","value":1.5}"#,
        r#"{"kind":"int","value":18446744073709551615}"#,
        r#"{"kind":"function","value":"f"}"#,
        r#"{"kind":"none","source":"forged"}"#,
        r#"{"kind":"content","value":{"name":"x","args":{},"attributes":{},"offset":5}}"#,
    ] {
        assert!(serde_json::from_str::<abi::Value>(json).is_err(), "{json}");
    }
}
