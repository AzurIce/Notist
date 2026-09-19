use serde_json::Value as Json;
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

use crate::{
    content::{Content, Item, Location},
    runtime::Value,
    syntax::Type,
};

const MAX_BYTES: usize = 1024 * 1024;

/// Experimental ABI: alloc(i32) -> i32, export(ptr, len) -> i64.
/// Input is a JSON array. Output is JSON, with pointer in the high 32 bits.
/// A fresh, import-free instance per call prevents hidden state between calls.
pub fn invoke(
    bytes: &[u8],
    export: &str,
    args: &[Value],
    result: &Type,
    location: &Location,
) -> Result<Value, String> {
    if !args.iter().all(Value::serializable) {
        return Err("WASM arguments must be data".into());
    }
    let input = serde_json::to_vec(&args.iter().map(Value::to_json).collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    let bytes = run(bytes, export, &input).map_err(|e| e.to_string())?;
    let value = decode(
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid result JSON: {e}"))?,
        location,
    )?;
    if matches!(value, Value::Content(Content::Error { .. }))
        || crate::runtime::matches_type(result, &value)
    {
        Ok(value)
    } else {
        Err(format!(
            "declared result {result:?}, received {:?}",
            value.ty()
        ))
    }
}

pub fn registration(bytes: &[u8]) -> Result<Json, String> {
    let output = run(bytes, "notist_register", b"[]").map_err(|e| e.to_string())?;
    serde_json::from_slice(&output).map_err(|e| format!("invalid registration JSON: {e}"))
}

pub fn validate_export(bytes: &[u8], export: &str) -> Result<(), String> {
    let engine = Engine::default();
    let module = Module::new(&engine, bytes).map_err(|e| e.to_string())?;
    let ty = module
        .get_export(export)
        .and_then(|e| e.func().cloned())
        .ok_or_else(|| format!("missing function export `{export}`"))?;
    if ty.params() != [wasmi::ValType::I32, wasmi::ValType::I32]
        || ty.results() != [wasmi::ValType::I64]
    {
        return Err(format!("invalid ABI for `{export}`"));
    }
    Ok(())
}

fn run(bytes: &[u8], export: &str, input: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if input.len() > MAX_BYTES || bytes.len() > 16 * MAX_BYTES {
        return Err("WASM input or module exceeds byte limit".into());
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, bytes)?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(16 * MAX_BYTES)
        .memories(1)
        .table_elements(4096)
        .tables(1)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits);
    store.set_fuel(100_000)?;
    let linker = Linker::new(&engine);
    let instance = linker.instantiate_and_start(&mut store, &module)?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("missing memory export")?;
    let alloc = instance.get_typed_func::<i32, i32>(&store, "alloc")?;
    let function = instance.get_typed_func::<(i32, i32), i64>(&store, export)?;
    let pointer = alloc.call(&mut store, input.len() as i32)?;
    memory.write(&mut store, pointer as u32 as usize, input)?;
    let packed = function.call(&mut store, (pointer, input.len() as i32))? as u64;
    let pointer = (packed >> 32) as usize;
    let length = (packed as u32) as usize;
    if length > MAX_BYTES {
        return Err("WASM result exceeds byte limit".into());
    }
    let mut output = vec![0; length];
    memory.read(&store, pointer, &mut output)?;
    Ok(output)
}

pub(crate) fn decode(json: Json, location: &Location) -> Result<Value, String> {
    match json {
        Json::String(s) => Ok(Value::String(s)),
        Json::Bool(b) => Ok(Value::Bool(b)),
        Json::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .ok_or("expected signed integer".into()),
        Json::Array(values) => Ok(Value::List(
            values
                .into_iter()
                .map(|v| decode(v, location))
                .collect::<Result<_, _>>()?,
        )),
        Json::Object(mut object) => {
            if let Some(fields) = object.remove("dict") {
                let Json::Object(fields) = fields else {
                    return Err("expected dictionary object".into());
                };
                return Ok(Value::Dict(
                    fields
                        .into_iter()
                        .map(|(k, v)| Ok((k, decode(v, location)?)))
                        .collect::<Result<_, String>>()?,
                ));
            }
            if let Some(Json::String(message)) = object.remove("error") {
                return Ok(Value::Content(Content::Error {
                    message,
                    location: location.clone(),
                }));
            }
            if let Some(Json::String(text)) = object.remove("text") {
                return Ok(Value::Content(Content::Text(text)));
            }
            fn children(json: Json, location: &Location) -> Result<Vec<Content>, String> {
                let Json::Array(values) = json else {
                    return Err("expected content array".into());
                };
                values
                    .into_iter()
                    .map(|v| match decode(v, location)? {
                        Value::Content(c) => Ok(c),
                        Value::Item(i) => Ok(Content::Item(i)),
                        _ => Err("expected Content child".into()),
                    })
                    .collect()
            }
            if let Some(sequence) = object.remove("sequence") {
                return Ok(Value::Content(Content::Sequence(children(
                    sequence, location,
                )?)));
            }
            if let Some(Json::String(name)) = object.remove("item") {
                let fields = object
                    .remove("args")
                    .and_then(|v| v.as_object().cloned())
                    .ok_or("expected Item args object")?;
                let args = fields
                    .into_iter()
                    .map(|(k, v)| Ok((k, decode(v, location)?)))
                    .collect::<Result<_, String>>()?;
                return Ok(Value::Item(Item {
                    name,
                    args,
                    attributes: object
                        .remove("attributes")
                        .map(|value| {
                            value
                                .as_object()
                                .ok_or("expected Item attributes object")?
                                .iter()
                                .map(|(k, v)| Ok((k.clone(), decode(v.clone(), location)?)))
                                .collect::<Result<_, String>>()
                        })
                        .transpose()?
                        .unwrap_or_default(),
                    location: location.clone(),
                }));
            }
            Err("unknown value encoding; Dict requires a dict wrapper".into())
        }
        Json::Null => Ok(Value::None),
    }
}
