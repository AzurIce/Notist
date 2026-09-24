//! Conversion at the plugin boundary; renderer/debug JSON is a separate format.
use crate::{Content, Item, Location, Value};
use notist_model::abi;
use std::collections::BTreeMap;

fn encode_fields(fields: &BTreeMap<String, Value>) -> Result<BTreeMap<String, abi::Value>, String> {
    fields
        .iter()
        .map(|(k, v)| Ok((k.clone(), v.to_abi()?)))
        .collect()
}

fn decode_fields(
    fields: BTreeMap<String, abi::Value>,
    location: &Location,
) -> BTreeMap<String, Value> {
    fields
        .into_iter()
        .map(|(k, v)| (k, Value::from_abi(v, location)))
        .collect()
}

impl Value {
    pub fn to_abi(&self) -> Result<abi::Value, String> {
        Ok(match self {
            Self::String(v) => abi::Value::String(v.clone()),
            Self::Int(v) => abi::Value::Int(*v),
            Self::Bool(v) => abi::Value::Bool(*v),
            Self::Unit => abi::Value::Unit,
            Self::List(v) => {
                abi::Value::List(v.iter().map(Self::to_abi).collect::<Result<_, _>>()?)
            }
            Self::Dict(v) => abi::Value::Dict(encode_fields(v)?),
            Self::Content(v) => abi::Value::Content(v.to_abi()?),
            Self::Target(v) => abi::Value::Target(v.clone()),
            _ => return Err(format!("{:?} cannot cross the WASM boundary", self.ty())),
        })
    }

    pub fn from_abi(value: abi::Value, location: &Location) -> Self {
        match value {
            abi::Value::String(v) => Self::String(v),
            abi::Value::Int(v) => Self::Int(v),
            abi::Value::Bool(v) => Self::Bool(v),
            abi::Value::Unit => Self::Unit,
            abi::Value::List(v) => {
                Self::List(v.into_iter().map(|v| Self::from_abi(v, location)).collect())
            }
            abi::Value::Dict(v) => Self::Dict(decode_fields(v, location)),
            abi::Value::Content(v) => Self::Content(Content::from_abi(v, location)),
            abi::Value::Target(v) => Self::Target(v),
        }
    }
}

impl Item {
    fn to_abi(&self) -> Result<abi::Item, String> {
        Ok(abi::Item {
            name: self.name.clone(),
            args: encode_fields(&self.args)?,
            attributes: encode_fields(&self.attributes)?,
        })
    }

    fn from_abi(item: abi::Item, location: &Location) -> Self {
        Self {
            name: item.name,
            args: decode_fields(item.args, location),
            attributes: decode_fields(item.attributes, location),
            location: location.clone(),
            span: None,
            origin: Some(crate::CreationOrigin {
                node_id: None,
                kind: crate::OriginKind::Plugin,
            }),
        }
    }
}
