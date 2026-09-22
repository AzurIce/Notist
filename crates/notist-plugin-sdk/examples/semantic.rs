use notist_plugin_sdk::{Content, ContentMode, ElementModel, Item, Value, func, init_plugin};
use std::collections::BTreeMap;

init_plugin!(elements = models; echo, add, optional, preferred, identity, paragraph, checked, badge);

fn models() -> BTreeMap<String, ElementModel> {
    BTreeMap::from([(
        "plugin-badge".into(),
        ElementModel {
            inline: true,
            block_field: None,
            slots: BTreeMap::from([("body".into(), ContentMode::Inline)]),
        },
    )])
}

#[func]
pub fn badge(body: Content) -> Content {
    Item::new(
        "plugin-badge",
        BTreeMap::from([("body".into(), Value::Content(body))]),
    )
}

#[func(defaults(source = "default".into()))]
pub fn echo(source: String) -> String {
    source
}

#[func(defaults(right = 1))]
pub fn add(left: i64, right: i64) -> i64 {
    left + right
}

#[func]
pub fn optional(value: Option<i64>) -> Option<i64> {
    value
}

#[func(defaults(value = Some(2)))]
pub fn preferred(value: Option<i64>) -> Option<i64> {
    value
}

#[func]
pub fn identity(value: Value) -> Value {
    value
}

#[func]
pub fn paragraph(body: Content) -> Item {
    Item {
        name: "paragraph".into(),
        args: BTreeMap::from([("body".into(), Value::Content(body))]),
        attributes: BTreeMap::new(),
    }
}

#[func]
pub fn checked(value: i64) -> Result<i64, String> {
    if value < 0 {
        Err("expected nonnegative value".into())
    } else {
        Ok(value)
    }
}
