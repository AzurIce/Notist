use notist_ir::{Content, Location, Target, Type, Value};
use serde_json::json;

#[test]
fn structured_targets_preserve_wire_encoding() {
    for labels in [vec![], vec!["章节".to_owned(), "例子".to_owned()]] {
        let target = Target::new("root::guide", labels.clone());
        let value = Value::Target(target.clone());
        assert_eq!(value.ty(), Type::Target);
        assert!(!value.serializable());
        assert_eq!(
            value.to_json(),
            json!({"target":"root::guide", "labels":labels})
        );
        let content = Content::Link {
            target: target.clone(),
            location: Location {
                source: "main.not".into(),
                offset: 5,
            },
        };
        assert_eq!(
            content.to_json(),
            json!({"link":target.to_string(), "target":target, "source":"main.not", "offset":5})
        );
    }
}
