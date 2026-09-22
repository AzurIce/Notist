use notist_ir::{Content, Location, Target, Type, Value};
use serde_json::json;

#[test]
fn structured_targets_preserve_wire_encoding() {
    for labels in [vec![], vec!["章节".to_owned(), "例子".to_owned()]] {
        let target = Target::new("root::guide", labels.clone());
        let value = Value::Target(target.clone());
        assert_eq!(value.ty(), Type::Target);
        assert!(value.serializable());
        assert_eq!(
            Value::from_abi(
                value.to_abi().unwrap(),
                &Location {
                    source: "call".into(),
                    offset: 0
                }
            )
            .to_json(),
            value.to_json()
        );
        assert_eq!(
            value.to_json(),
            json!({"target":"root::guide", "labels":labels})
        );
        let content = Content::link(
            target.clone(),
            Location {
                source: "main.not".into(),
                offset: 5,
            },
        );
        assert_eq!(
            content.to_json(),
            json!({"item":"link", "args":{"target":{"target":target.module,"labels":target.labels}}, "attributes":{}, "label":null, "source":"main.not", "offset":5})
        );
    }
}
