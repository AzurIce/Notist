use notist_ir::{Content, Target, Type, Value};
use serde_json::json;

#[test]
fn structured_targets_preserve_wire_encoding() {
    for item in [None, Some("intro".to_owned())] {
        let target = Target::new("vault::guide", item.clone());
        let value = Value::Target(target.clone());
        assert_eq!(value.ty(), Type::Target);
        assert!(!value.serializable());
        assert_eq!(
            value.to_json(),
            json!({"target":"vault::guide", "item":item})
        );
        let content = Content::Link {
            target: target.clone(),
        };
        assert_eq!(content.to_json(), json!({"link":target.to_string()}));
    }
}
