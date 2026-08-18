//! Runtime Wasm plugin host for Notist.
//!
//! This loader reads `Notist.toml`, resolves plugin package directories,
//! reads `plugin.json`, and loads a WebAssembly module with Wasmtime.
//!
//! The current Wasm ABI is intentionally minimal:
//!
//! ```text
//! evaluate(ptr: i32, len: i32) -> i32
//! ```
//!
//! The host writes a binary request into Wasm memory, calls `evaluate`, and
//! reads a NUL-terminated JSON response from the returned pointer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notist_eval::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
    FunctionSignature, RegistryError, Type,
};
use notist_model::{Content, CustomField, DefaultValue, Element, ElementValue, Parameter};
use serde::Deserialize;
use wasmtime::{Engine, Func, Instance, Memory, Module, Store, Val};

/// A plugin entry in `Notist.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct VaultPluginConfig {
    /// Path to the plugin package directory, relative to the vault root.
    pub path: Option<String>,
    /// A future registry package name.
    pub package: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VaultConfig {
    #[serde(default)]
    plugins: BTreeMap<String, VaultPluginConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginManifest {
    pub package: String,
    pub version: String,
    #[serde(rename = "api-version")]
    pub api_version: String,
    #[serde(default)]
    pub wasm: Option<WasmDecl>,
    #[serde(default)]
    pub interfaces: ManifestInterfaces,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WasmDecl {
    pub module: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ManifestInterfaces {
    #[serde(default)]
    pub semantic: Option<SemanticInterface>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SemanticInterface {
    #[serde(default)]
    pub elements: Vec<ElementDecl>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ElementDecl {
    pub name: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default = "default_true")]
    pub block: bool,
    #[serde(default)]
    pub parameters: Vec<ParamDecl>,
    #[serde(rename = "trailing-content")]
    pub trailing_content: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    pub ty: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// A plugin loaded from a Wasm module.
pub struct LoadedPlugin {
    pub id: String,
    pub functions: Vec<Arc<dyn Function>>,
}

/// Returns plugin package directories declared in `Notist.toml`, without loading Wasm.
pub fn plugin_package_dirs(
    root: &Path,
    toml_text: Option<&str>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let Some(toml_text) = toml_text else {
        return Ok(Vec::new());
    };
    let config: VaultConfig = toml::from_str(toml_text)
        .map_err(|error| format!("invalid Notist.toml: {error}"))?;
    let mut packages = Vec::new();
    for (name, entry) in config.plugins {
        let package_dir = match &entry.path {
            Some(path) => root.join(path),
            None => {
                return Err(format!("plugin `{name}` must declare a `path` in Notist.toml"));
            }
        };
        packages.push((name, package_dir));
    }
    Ok(packages)
}

/// Loads all plugins declared in `Notist.toml`.
pub fn load_plugins_from_vault(
    root: &Path,
    toml_text: Option<&str>,
) -> Result<Vec<LoadedPlugin>, String> {
    let Some(toml_text) = toml_text else {
        return Ok(Vec::new());
    };
    let config: VaultConfig = toml::from_str(toml_text)
        .map_err(|error| format!("invalid Notist.toml: {error}"))?;
    let mut loaded = Vec::new();
    for (name, entry) in config.plugins {
        let package_dir = match &entry.path {
            Some(path) => root.join(path),
            None => {
                return Err(format!("plugin `{name}` must declare a `path` in Notist.toml"));
            }
        };
        let plugin = load_package(&package_dir)?;
        loaded.push(plugin);
    }
    Ok(loaded)
}

/// Loads one plugin package directory.
pub fn load_package(package_dir: &Path) -> Result<LoadedPlugin, String> {
    let manifest_path = package_dir.join("plugin.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&manifest_text)
        .map_err(|error| format!("invalid plugin.json: {error}"))?;

    let wasm = manifest
        .wasm
        .as_ref()
        .ok_or_else(|| format!("plugin `{}` has no wasm module", manifest.package))?;
    let wasm_path = package_dir.join(&wasm.module);
    let wasm_bytes = std::fs::read(&wasm_path)
        .map_err(|error| format!("cannot read {}: {error}", wasm_path.display()))?;

    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes)
        .map_err(|error| format!("invalid wasm module {}: {error}", wasm_path.display()))?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])
        .map_err(|error| format!("cannot instantiate {}: {error}", wasm_path.display()))?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| format!("plugin `{}` does not export memory", manifest.package))?;
    let evaluate = instance
        .get_func(&mut store, "evaluate")
        .ok_or_else(|| format!("plugin `{}` does not export evaluate", manifest.package))?;

    let runtime = Arc::new(Mutex::new(WasmRuntime {
        store,
        instance,
        memory,
        evaluate,
    }));

    let semantic = manifest.interfaces.semantic.as_ref();
    let mut functions = Vec::new();
    if let Some(semantic) = semantic {
        for element in &semantic.elements {
            let signature = element_signature(element)?;
            functions.push(Arc::new(WasmFunction {
                element_name: element.name.clone(),
                block: element.block,
                signature,
                runtime: Arc::clone(&runtime),
            }) as Arc<dyn Function>);
        }
    }

    Ok(LoadedPlugin {
        id: manifest.package,
        functions,
    })
}

fn element_signature(element: &ElementDecl) -> Result<FunctionSignature, String> {
    let mut parameters = Vec::new();
    for param in &element.parameters {
        let ty = parse_type(&param.ty)?;
        let default = match &param.default {
            Some(value) => Some(json_default(value)?),
            None => None,
        };
        parameters.push(Parameter {
            name: param.name.clone(),
            ty,
            default,
        });
    }
    if let Some(trailing) = &element.trailing_content {
        parameters.push(Parameter {
            name: trailing.clone(),
            ty: Type::Content,
            default: None,
        });
    }
    Ok(FunctionSignature {
        parameters,
        trailing_content: element.trailing_content.clone(),
        result: Type::Content,
    })
}

fn parse_type(ty: &str) -> Result<Type, String> {
    match ty {
        "None" => Ok(Type::None),
        "Bool" => Ok(Type::Bool),
        "Int" => Ok(Type::Int),
        "Float" => Ok(Type::Float),
        "String" => Ok(Type::String),
        "Content" => Ok(Type::Content),
        _ if ty.ends_with('?') => Ok(Type::Optional(Box::new(parse_type(&ty[..ty.len() - 1])?))),
        _ => Err(format!("unsupported plugin parameter type `{ty}`")),
    }
}

fn json_default(value: &serde_json::Value) -> Result<DefaultValue, String> {
    match value {
        serde_json::Value::Null => Ok(DefaultValue::None),
        serde_json::Value::Bool(value) => Ok(DefaultValue::Bool(*value)),
        serde_json::Value::Number(value) if value.is_i64() => {
            Ok(DefaultValue::Int(value.as_i64().unwrap()))
        }
        serde_json::Value::Number(value) if value.is_f64() => {
            Ok(DefaultValue::Float(value.as_f64().unwrap()))
        }
        serde_json::Value::String(value) => Ok(DefaultValue::String(value.clone())),
        _ => Err(format!("unsupported plugin default {value}")),
    }
}

struct WasmRuntime {
    store: Store<()>,
    #[allow(dead_code)]
    instance: Instance,
    memory: Memory,
    evaluate: Func,
}

struct WasmFunction {
    element_name: String,
    block: bool,
    signature: FunctionSignature,
    runtime: Arc<Mutex<WasmRuntime>>,
}

impl Function for WasmFunction {
    fn name(&self) -> &str {
        &self.element_name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let mut request = Vec::new();
        for param in &self.signature.parameters {
            if param.ty == Type::Content {
                continue;
            }
            let Some(value) = input.arguments.get(&param.name) else {
                continue;
            };
            encode_value(&mut request, value);
        }

        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| vec![EvalDiagnostic {
                message: "plugin runtime lock poisoned".into(),
                range: input.range,
            }])?;

        const INPUT_OFFSET: usize = 1024;
        let WasmRuntime {
            store,
            memory,
            evaluate,
            ..
        } = &mut *runtime;
        if memory.data_size(&mut *store) < INPUT_OFFSET + request.len() {
            return Err(vec![EvalDiagnostic {
                message: "plugin request does not fit in wasm memory".into(),
                range: input.range,
            }]);
        }
        memory
            .write(&mut *store, INPUT_OFFSET, &request)
            .map_err(|error| vec![EvalDiagnostic {
                message: format!("cannot write wasm memory: {error}"),
                range: input.range,
            }])?;

        let mut results = [Val::I32(0)];
        evaluate
            .call(
                &mut *store,
                &[Val::I32(INPUT_OFFSET as i32), Val::I32(request.len() as i32)],
                &mut results,
            )
            .map_err(|error| vec![EvalDiagnostic {
                message: format!("wasm plugin error: {error}"),
                range: input.range,
            }])?;
        let Val::I32(response_ptr) = results[0] else {
            return Err(vec![EvalDiagnostic {
                message: "wasm plugin returned non-i32".into(),
                range: input.range,
            }]);
        };

        let mut response = [0u8; 9];
        memory
            .read(&mut *store, response_ptr as usize, &mut response)
            .map_err(|error| vec![EvalDiagnostic {
                message: format!("cannot read wasm response: {error}"),
                range: input.range,
            }])?;
        if response[0] != 1 {
            return Err(vec![EvalDiagnostic {
                message: "wasm plugin returned ok=false".into(),
                range: input.range,
            }]);
        }
        let width = i32::from_le_bytes(response[1..5].try_into().unwrap());
        let height = i32::from_le_bytes(response[5..9].try_into().unwrap());

        let body = input.arguments.take_content(
            self.signature
                .trailing_content
                .as_deref()
                .unwrap_or("body"),
        );
        let mut fields = BTreeMap::new();
        for param in &self.signature.parameters {
            if param.ty == Type::Content {
                continue;
            }
            if let Some(value) = input.arguments.get(&param.name) {
                fields.insert(param.name.clone(), value_to_json(value));
            }
        }
        fields.insert("width".to_string(), serde_json::Value::Number(width.into()));
        fields.insert("height".to_string(), serde_json::Value::Number(height.into()));
        let fields = fields
            .into_iter()
            .map(|(name, value)| CustomField {
                name,
                value: json_to_element_value(value),
            })
            .collect();
        Ok(FunctionOutput::content(Content::single(
            Element::Custom {
                name: self.element_name.clone(),
                body,
                block: self.block,
                fields,
            },
            input.range,
        )))
    }
}

fn encode_value(request: &mut Vec<u8>, value: &notist_eval::Value) {
    match value {
        notist_eval::Value::String(value) => {
            let bytes = value.as_bytes();
            request.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            request.extend_from_slice(bytes);
        }
        notist_eval::Value::Int(value) => {
            request.extend_from_slice(&(*value as i32).to_le_bytes());
        }
        notist_eval::Value::Bool(value) => {
            request.push(u8::from(*value));
        }
        notist_eval::Value::Float(value) => {
            request.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        _ => {}
    }
}

fn value_to_json(value: &notist_eval::Value) -> serde_json::Value {
    match value {
        notist_eval::Value::None => serde_json::Value::Null,
        notist_eval::Value::Bool(value) => serde_json::Value::Bool(*value),
        notist_eval::Value::Int(value) => serde_json::Value::Number((*value).into()),
        notist_eval::Value::Float(value) => {
            serde_json::Number::from_f64(*value).map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        notist_eval::Value::String(value) => serde_json::Value::String(value.clone()),
        notist_eval::Value::Content(_) | notist_eval::Value::Function(_) => serde_json::Value::Null,
    }
}

fn json_to_element_value(value: serde_json::Value) -> ElementValue {
    match value {
        serde_json::Value::Null => ElementValue::None,
        serde_json::Value::Bool(value) => ElementValue::Bool(value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                ElementValue::Int(value)
            } else if let Some(value) = value.as_f64() {
                ElementValue::Float(value)
            } else {
                ElementValue::None
            }
        }
        serde_json::Value::String(value) => ElementValue::String(value),
        serde_json::Value::Array(values) => {
            ElementValue::Array(values.into_iter().map(json_to_element_value).collect())
        }
        serde_json::Value::Object(_) => ElementValue::None,
    }
}

/// Registers loaded plugin functions into a registry.
pub fn register_loaded(
    registry: &mut FunctionRegistry,
    plugins: &[LoadedPlugin],
) -> Result<(), RegistryError> {
    for plugin in plugins {
        for function in &plugin.functions {
            registry.register_arc(Arc::clone(function))?;
        }
    }
    Ok(())
}
