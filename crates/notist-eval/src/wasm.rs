use wasmi::{Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

use crate::{Env, Value, matches_type};
use notist_ir::External;
use notist_model::{Location, Type, abi};
use notist_syntax::Param;
use std::rc::Rc;

const MAX_BYTES: usize = 1024 * 1024;
// The budget includes Rust SDK registration and payload conversion.
const MAX_FUEL: u64 = 1_000_000;

#[cfg(test)]
#[path = "wasm_tests.rs"]
mod tests;

/// alloc(i32) -> i32, export(ptr, len) -> i64, pointer in the high 32 bits.
/// Payloads use the shared ABI types, not the renderer's JSON projection.
/// A fresh, import-free instance per call prevents hidden state between calls.
pub fn invoke(
    bytes: &[u8],
    export: &str,
    args: &[Value],
    result: &Type,
    location: &Location,
) -> Result<Value, String> {
    let args = args
        .iter()
        .map(Value::to_abi)
        .collect::<Result<Vec<_>, _>>()?;
    let input = serde_json::to_vec(&args).map_err(|e| e.to_string())?;
    let output = run(bytes, export, &input).map_err(|e| e.to_string())?;
    let value = Value::from_abi(
        serde_json::from_slice(&output).map_err(|e| format!("invalid result encoding: {e}"))?,
        location,
    );
    if value.is_error() || matches_type(result, &value) {
        Ok(value)
    } else {
        Err(format!(
            "declared result {result:?}, received {:?}",
            value.ty()
        ))
    }
}

pub fn registration(bytes: &[u8], path: &str, location: &Location) -> Result<Env, String> {
    let output = run(bytes, "notist_register", b"[]").map_err(|e| e.to_string())?;
    let registry: abi::Registration = serde_json::from_slice(&output)
        .map_err(|e| format!("invalid registration encoding: {e}"))?;
    let engine = Engine::default();
    let module = Module::new(&engine, bytes).map_err(|e| e.to_string())?;
    let mut env = Env::new();
    for (name, desc) in registry.functions {
        if !notist_syntax::valid_binding(&name) {
            return Err(format!("invalid registered name `{name}`"));
        }
        validate_type(&desc.result)?;
        validate_export(&module, &desc.export)?;
        let mut params = Vec::new();
        let mut defaults = Env::new();
        for p in desc.params {
            if !notist_syntax::valid_binding(&p.name)
                || params.iter().any(|param: &Param| param.name == p.name)
            {
                return Err(format!("invalid or duplicate parameter name `{}`", p.name));
            }
            validate_type(&p.ty)?;
            if let Some(default) = p.default {
                let value = Value::from_abi(default, location);
                if !matches_type(&p.ty, &value) {
                    return Err(format!("default type mismatch for `{}`", p.name));
                }
                defaults.insert(p.name.clone(), value);
            }
            params.push(Param {
                name: p.name,
                ty: p.ty,
                default: None,
            });
        }
        env.insert(
            name,
            Value::External(Rc::new(External {
                params,
                defaults,
                path: path.into(),
                export: desc.export,
                result: desc.result,
            })),
        );
    }
    Ok(env)
}

fn validate_type(ty: &Type) -> Result<(), String> {
    match ty {
        Type::Module | Type::Target | Type::Function => {
            Err(format!("{ty:?} cannot cross the WASM boundary"))
        }
        Type::Optional(inner) => validate_type(inner),
        _ => Ok(()),
    }
}

fn validate_export(module: &Module, export: &str) -> Result<(), String> {
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
    store.set_fuel(MAX_FUEL)?;
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
