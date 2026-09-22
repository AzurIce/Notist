use notist_plugin_sdk::{Content, PluginValue, Type, Value, abi};
use std::collections::BTreeMap;

#[path = "../examples/semantic.rs"]
mod semantic;

mod collections {
    use super::*;
    notist_plugin_sdk::init_plugin!(sum, dict, nothing, same);

    #[notist_plugin_sdk::func]
    pub fn sum(values: Vec<i64>) -> i64 {
        values.into_iter().sum()
    }
    #[notist_plugin_sdk::func]
    pub fn dict(values: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        values
    }
    #[notist_plugin_sdk::func]
    pub fn nothing() {}
    #[notist_plugin_sdk::func]
    pub fn same(same: String) -> String {
        same
    }
}

mod paths {
    type Number = i64;
    mod nested {
        type Text = String;

        #[notist_plugin_sdk::func]
        pub fn qualified(number: super::Number, text: self::Text) -> String {
            format!("{number}:{text}")
        }
    }
    notist_plugin_sdk::init_plugin!(nested::qualified);
}

fn invoke(export: &str, args: Vec<Value>) -> Value {
    serde_json::from_slice(&semantic::notist_dispatch(
        export,
        &serde_json::to_vec(&args).unwrap(),
    ))
    .unwrap()
}

#[test]
fn rust_signatures_produce_typed_registration() {
    let registry = semantic::notist_registration();
    assert!(registry.elements["plugin-badge"].inline);
    assert_eq!(
        registry.elements["plugin-badge"].slots["body"],
        notist_plugin_sdk::ContentMode::Inline
    );
    assert_eq!(registry.functions["badge"].result, Type::Content);
    assert_eq!(registry.functions["add"].params[0].ty, Type::Int);
    assert_eq!(
        registry.functions["add"].params[1].default,
        Some(Value::Int(1))
    );
    assert_eq!(registry.functions["checked"].result, Type::Int);
    let optional = &registry.functions["optional"].params[0];
    assert_eq!(optional.ty, Type::Optional(Box::new(Type::Int)));
    assert_eq!(optional.default, None);
    assert_eq!(
        registry.functions["preferred"].params[0].default,
        Some(Value::Int(2))
    );
    assert_eq!(registry.functions["identity"].params[0].default, None);
    let bytes = serde_json::to_vec(&registry).unwrap();
    assert_eq!(
        serde_json::from_slice::<abi::Registration>(&bytes).unwrap(),
        registry
    );
    assert_eq!(
        collections::notist_registration().functions["nothing"].result,
        Type::None
    );
}

#[test]
fn wrappers_convert_values_and_errors() {
    assert_eq!(
        invoke("add", vec![Value::Int(2), Value::Int(3)]),
        Value::Int(5)
    );
    assert_eq!(invoke("optional", vec![Value::None]), Value::None);
    assert_eq!(invoke("optional", vec![Value::Int(4)]), Value::Int(4));
    assert_eq!(
        invoke("checked", vec![Value::Int(-1)]),
        Value::Content(Content::error("expected nonnegative value"))
    );
    let item = notist_plugin_sdk::Item {
        name: "custom".into(),
        args: BTreeMap::new(),
        attributes: BTreeMap::new(),
    };
    assert!(matches!(
        invoke("paragraph", vec![Value::Content(item)]),
        Value::Content(_)
    ));
    for (export, args) in [
        ("add", vec![]),
        ("echo", vec![Value::Int(1)]),
        ("echo", vec![Value::None, Value::None]),
        ("missing", vec![]),
    ] {
        assert!(matches!(
            invoke(export, args),
            Value::Content(c) if c.name == "error"
        ));
    }
    let invalid: Value =
        serde_json::from_slice(&semantic::notist_dispatch("echo", b"not json")).unwrap();
    assert!(matches!(invalid, Value::Content(c) if c.name == "error"));
}

#[test]
fn collection_conversions_check_members() {
    let input = vec![2i64, 3].into_value();
    let result: Value = serde_json::from_slice(&collections::notist_dispatch(
        "sum",
        &serde_json::to_vec(&vec![input]).unwrap(),
    ))
    .unwrap();
    assert_eq!(result, Value::Int(5));
    assert!(Vec::<i64>::from_value(Value::List(vec![Value::None])).is_err());
    let dict = BTreeMap::from([("x".into(), Value::List(vec![Value::Bool(true)]))]);
    assert_eq!(
        BTreeMap::<String, Value>::from_value(dict.clone().into_value()).unwrap(),
        dict
    );
}

#[test]
fn argument_names_do_not_shadow_the_exported_function() {
    let args = serde_json::to_vec(&vec![Value::String("same".into())]).unwrap();
    let result: Value =
        serde_json::from_slice(&collections::notist_dispatch("same", &args)).unwrap();
    assert_eq!(result, Value::String("same".into()));
}

#[test]
fn module_paths_and_relative_types_keep_their_rust_meaning() {
    assert_eq!(
        paths::notist_registration().functions["qualified"].params[0].ty,
        Type::Int
    );
    let input = serde_json::to_vec(&vec![Value::Int(3), Value::String("text".into())]).unwrap();
    let result: Value =
        serde_json::from_slice(&paths::notist_dispatch("qualified", &input)).unwrap();
    assert_eq!(result, Value::String("3:text".into()));
    assert_eq!(semantic::optional(None), None);
    assert_eq!(semantic::preferred(None), None);
    assert_eq!(semantic::add(3, 4), 7);
}
