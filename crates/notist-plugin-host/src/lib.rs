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
//! reads the plugin response from the returned pointer.
//!
//! The checked-in shader package uses the v0 binary response
//! (`ok: u8`, `width: i32`, `height: i32`). Declarative element packages do
//! not need a Wasm module at all and use [`ElementFunction`] instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notist_eval::{
    ElementFunction, EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOutput,
    FunctionOwner, FunctionRegistry, FunctionSignature, Principal, RegistryError, ShapingRegistry,
    Type,
};
use notist_model::{
    BodyMode, Content, CustomField, DefaultValue, Element, ElementName, ElementSchema,
    ElementValue, Parameter, ShapingKind, ShapingRole,
};
use serde::Deserialize;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Func, Instance, Memory, Module, ResourceLimiter, Store, Val};

/// A plugin entry in `Notist.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct VaultPluginConfig {
    /// Path to the plugin package directory, relative to the vault root.
    pub path: Option<String>,
    /// A future registry package name.
    pub package: Option<String>,
    /// Site-granted capabilities for this plugin. Effective capabilities are
    /// the intersection of this set with `plugin.json` `capabilities.request`.
    #[serde(default)]
    pub capabilities: Vec<String>,
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
    #[serde(default)]
    pub capabilities: ManifestCapabilities,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WasmDecl {
    pub module: String,
    /// When true, `module` is a WIT component rather than a raw core Wasm
    /// module using the legacy v0 ABI.
    #[serde(default)]
    pub component: bool,
    /// When true, the component imports `host.call` and is instantiated with
    /// the shared plugin registry wired into that import.
    #[serde(rename = "host-call", default)]
    pub host_call: bool,
}

/// The capability declaration used by the plugin host.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ManifestCapabilities {
    /// Qualified function names this package may request through `host.call`.
    #[serde(default)]
    pub request: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ManifestInterfaces {
    #[serde(default)]
    pub semantic: Option<SemanticInterface>,
    #[serde(default)]
    pub render: Option<ManifestRender>,
}

/// Target-keyed renderer contributions declared by a package.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ManifestRender {
    #[serde(default)]
    pub html: Option<ManifestHtmlRender>,
}

/// HTML target renderer contributions.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ManifestHtmlRender {
    #[serde(default)]
    pub contributions: Vec<HtmlContribution>,
}

/// One HTML renderer contribution.
#[derive(Clone, Debug, Deserialize)]
pub struct HtmlContribution {
    /// The plugin element this contribution renders.
    pub element: String,
    /// Whether this contribution may emit trusted HTML/scripts.
    #[serde(default)]
    pub trusted: bool,
    /// Declarative Web Component projection, when present.
    #[serde(rename = "web-component", default)]
    pub web_component: Option<WebComponentDecl>,
}

/// A declarative custom-element projection.
#[derive(Clone, Debug, Deserialize)]
pub struct WebComponentDecl {
    pub tag: String,
    pub module: String,
    #[serde(default)]
    pub style: Option<String>,
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
    #[serde(rename = "body-mode", default)]
    pub body_mode: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    /// Optional explicit shaping kind. Declarative elements default to
    /// `block | inline`; `separator` is used by `core::parbreak`.
    #[serde(default)]
    pub kind: Option<String>,
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

/// Manifest-declared HTML assets for one package entry in `Notist.toml`.
#[derive(Clone, Debug)]
pub struct PluginHtmlAssets {
    /// The `Notist.toml` key, which also names the copied asset directory.
    pub name: String,
    /// HTML contributions declared by the package manifest.
    pub contributions: Vec<HtmlContribution>,
}

/// A plugin package after manifest validation and module instantiation.
pub struct LoadedPlugin {
    /// Manifest package id, also used as the element namespace.
    pub id: String,
    /// Manifest package version.
    pub version: String,
    /// Manifest API version.
    pub api_version: String,
    /// Eval-side contributions, either declarative or Wasm-backed.
    pub functions: Vec<Arc<dyn Function>>,
    /// Shaping schemas for each declared element.
    pub elements: Vec<ElementSchema>,
    /// HTML renderer contributions declared by the manifest.
    pub html_contributions: Vec<HtmlContribution>,
    /// Capabilities requested by the manifest. Site grants are not applied
    /// here; consumers intersect this set with their configured grants.
    pub capabilities: Vec<String>,
    /// Registry shared with component `host.call` imports. It already
    /// contains core functions, this package's functions, and its requested
    /// grants by the time `load_package` returns.
    pub shared_registry: Option<Arc<Mutex<FunctionRegistry>>>,
}

/// Returns plugin package directories declared in `Notist.toml`, without loading Wasm.
pub fn plugin_package_dirs(
    root: &Path,
    toml_text: Option<&str>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let Some(toml_text) = toml_text else {
        return Ok(Vec::new());
    };
    let config: VaultConfig =
        toml::from_str(toml_text).map_err(|error| format!("invalid Notist.toml: {error}"))?;
    let mut packages = Vec::new();
    for (name, entry) in config.plugins {
        let package_dir = match &entry.path {
            Some(path) => root.join(path),
            None => {
                return Err(format!(
                    "plugin `{name}` must declare a `path` in Notist.toml"
                ));
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
    let config: VaultConfig =
        toml::from_str(toml_text).map_err(|error| format!("invalid Notist.toml: {error}"))?;
    let mut loaded = Vec::new();
    for (name, entry) in config.plugins {
        let package_dir = match &entry.path {
            Some(path) => root.join(path),
            None => {
                return Err(format!(
                    "plugin `{name}` must declare a `path` in Notist.toml"
                ));
            }
        };
        let plugin = load_package_with_grants(&package_dir, Some(&entry.capabilities))?;
        loaded.push(plugin);
    }
    Ok(loaded)
}

/// Reads and validates one `plugin.json` without compiling or instantiating Wasm.
pub fn read_manifest(package_dir: &Path) -> Result<PluginManifest, String> {
    let manifest_path = package_dir.join("plugin.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&manifest_text)
        .map_err(|error| format!("invalid plugin.json: {error}"))?;
    validate_html_contributions(&manifest)?;
    Ok(manifest)
}

fn validate_html_contributions(manifest: &PluginManifest) -> Result<(), String> {
    let Some(html) = manifest
        .interfaces
        .render
        .as_ref()
        .and_then(|render| render.html.as_ref())
    else {
        return Ok(());
    };
    for contribution in &html.contributions {
        let Some(component) = &contribution.web_component else {
            continue;
        };
        let tag = component.tag.as_bytes();
        let valid = !tag.is_empty()
            && tag
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            && tag.contains(&b'-');
        if !valid {
            return Err(format!(
                "plugin `{}` declares invalid web-component tag `{}`",
                manifest.package, component.tag
            ));
        }
    }
    Ok(())
}

/// Collects manifest-declared HTML assets for packages named in `Notist.toml`.
///
/// This is intentionally a manifest-only operation; it does not compile or
/// instantiate Wasm modules.
pub fn plugin_html_assets(
    root: &Path,
    toml_text: Option<&str>,
) -> Result<Vec<PluginHtmlAssets>, String> {
    let mut assets = Vec::new();
    for (name, package_dir) in plugin_package_dirs(root, toml_text)? {
        let manifest = read_manifest(&package_dir)?;
        let contributions = manifest
            .interfaces
            .render
            .as_ref()
            .and_then(|render| render.html.as_ref())
            .map(|html| html.contributions.clone())
            .unwrap_or_default();
        assets.push(PluginHtmlAssets {
            name,
            contributions,
        });
    }
    Ok(assets)
}

/// Loads one plugin package directory.
///
/// Packages may be either Wasm-backed or purely declarative. A declarative
/// package contributes element schemas and signatures; the host projects
/// bound arguments through [`ElementFunction`] without executing guest code.
pub fn load_package(package_dir: &Path) -> Result<LoadedPlugin, String> {
    load_package_with_grants(package_dir, None)
}

/// Loads one plugin package with site-granted capabilities.
///
/// `None` preserves standalone-test compatibility by granting every requested
/// capability. Vault loading always passes the site grant set from
/// `Notist.toml`; effective permissions are the intersection with
/// `plugin.json` `capabilities.request`.
pub fn load_package_with_grants(
    package_dir: &Path,
    site_grants: Option<&[String]>,
) -> Result<LoadedPlugin, String> {
    let manifest = read_manifest(package_dir)?;
    let effective_capabilities = match site_grants {
        None => manifest.capabilities.request.clone(),
        Some(grants) => {
            if grants.iter().any(|grant| grant == "*") {
                manifest.capabilities.request.clone()
            } else {
                manifest
                    .capabilities
                    .request
                    .iter()
                    .filter(|requested| {
                        grants
                            .iter()
                            .any(|grant| grant.as_str() == requested.as_str())
                    })
                    .cloned()
                    .collect()
            }
        }
    };

    let shared_registry = if manifest
        .wasm
        .as_ref()
        .is_some_and(|wasm| wasm.component && wasm.host_call)
    {
        Some(Arc::new(Mutex::new(FunctionRegistry::with_builtins())))
    } else {
        None
    };
    let runtime = match &manifest.wasm {
        Some(wasm) => {
            let wasm_path = package_dir.join(&wasm.module);
            let wasm_bytes = std::fs::read(&wasm_path)
                .map_err(|error| format!("cannot read {}: {error}", wasm_path.display()))?;
            if wasm.component && wasm.host_call {
                Some(SemanticRuntime::ComponentHost(Arc::new(Mutex::new(
                    load_component_host_runtime(
                        &manifest.package,
                        &wasm_path,
                        &wasm_bytes,
                        Arc::clone(shared_registry.as_ref().unwrap()),
                    )?,
                ))))
            } else if wasm.component {
                Some(SemanticRuntime::Component(Arc::new(Mutex::new(
                    load_component_runtime(&manifest.package, &wasm_path, &wasm_bytes)?,
                ))))
            } else {
                Some(SemanticRuntime::Core(Arc::new(Mutex::new(
                    load_wasm_runtime(&manifest.package, &wasm_path, &wasm_bytes)?,
                ))))
            }
        }
        None => None,
    };

    let semantic = manifest.interfaces.semantic.as_ref();
    let mut functions = Vec::new();
    let mut elements = Vec::new();
    if let Some(semantic) = semantic {
        for element in &semantic.elements {
            let signature = element_signature(element)?;
            let element_name = plugin_element_name(&manifest.package, &element.name);
            let owner = FunctionOwner::Plugin(manifest.package.clone());
            let function: Arc<dyn Function> = match &runtime {
                Some(SemanticRuntime::Core(runtime)) => Arc::new(WasmFunction {
                    element_name: element_name.clone(),
                    block: element.block,
                    signature,
                    runtime: Arc::clone(runtime),
                    owner,
                }),
                Some(SemanticRuntime::Component(runtime)) => Arc::new(ComponentFunction {
                    element_name: element_name.clone(),
                    signature,
                    runtime: Arc::clone(runtime),
                    owner,
                }),
                Some(SemanticRuntime::ComponentHost(runtime)) => Arc::new(ComponentHostFunction {
                    element_name: element_name.clone(),
                    signature,
                    runtime: Arc::clone(runtime),
                    owner,
                }),
                None => Arc::new(ElementFunction::new(
                    element_name.clone(),
                    signature,
                    element.block,
                    owner,
                )),
            };
            functions.push(function);
            elements.push(element_schema(&element_name, element, &manifest.package)?);
        }
    }

    if let Some(shared) = &shared_registry {
        let mut registry = shared.lock().map_err(|_| {
            format!(
                "plugin `{}` shared registry lock poisoned",
                manifest.package
            )
        })?;
        for function in &functions {
            registry
                .register_arc(Arc::clone(function))
                .map_err(|error| format!("cannot register shared function: {error:?}"))?;
            let Some((package, element)) = function.name().split_once("::") else {
                continue;
            };
            if let Some(alias) = plugin_legacy_alias(package, element) {
                registry
                    .register_alias(alias, function.name())
                    .map_err(|error| format!("cannot register shared alias: {error:?}"))?;
            }
        }
        let principal = Principal::Plugin(manifest.package.clone());
        for capability in &effective_capabilities {
            registry.allow(principal.clone(), capability.clone());
        }
    }

    let html_contributions = manifest
        .interfaces
        .render
        .as_ref()
        .and_then(|render| render.html.as_ref())
        .map(|html| html.contributions.clone())
        .unwrap_or_default();

    Ok(LoadedPlugin {
        id: manifest.package.clone(),
        version: manifest.version,
        api_version: manifest.api_version,
        functions,
        elements,
        html_contributions,
        capabilities: effective_capabilities,
        shared_registry,
    })
}

/// Fuel granted to one plugin dispatch. With `consume_fuel` enabled this
/// bounds even non-terminating Wasm loops without adding a timer thread.
const WASM_FUEL_PER_CALL: u64 = 10_000_000;

/// Maximum encoded bytes accepted from one component response.
const MAX_COMPONENT_RESPONSE_BYTES: usize = 1024 * 1024;

/// Maximum nodes accepted in one component response stream.
const MAX_COMPONENT_RESPONSE_NODES: usize = 10_000;

/// Maximum linear memory a plugin may allocate.
const WASM_MAX_MEMORY_BYTES: usize = 16 * 1024 * 1024;

/// Maximum table elements a plugin may allocate.
const WASM_MAX_TABLE_ELEMENTS: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
struct WasmStoreState {
    max_memory: usize,
    max_table_elements: usize,
}

impl ResourceLimiter for WasmStoreState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_memory)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_table_elements)
    }
}

fn load_wasm_runtime(
    package: &str,
    wasm_path: &Path,
    wasm_bytes: &[u8],
) -> Result<WasmRuntime, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("cannot create wasm engine for `{package}`: {error}"))?;
    let module = Module::new(&engine, wasm_bytes)
        .map_err(|error| format!("invalid wasm module {}: {error}", wasm_path.display()))?;
    let mut store = Store::new(
        &engine,
        WasmStoreState {
            max_memory: WASM_MAX_MEMORY_BYTES,
            max_table_elements: WASM_MAX_TABLE_ELEMENTS,
        },
    );
    store.limiter(|state| state);
    let instance = Instance::new(&mut store, &module, &[])
        .map_err(|error| format!("cannot instantiate {}: {error}", wasm_path.display()))?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| format!("plugin `{package}` does not export memory"))?;
    let evaluate = instance
        .get_func(&mut store, "evaluate")
        .ok_or_else(|| format!("plugin `{package}` does not export evaluate"))?;
    Ok(WasmRuntime {
        store,
        instance,
        memory,
        evaluate,
    })
}

/// Returns the qualified call-site and element name for a manifest element.
///
/// All plugin elements are namespaced by their package (`shader::shader`,
/// `demo::box`). The v1 shader spelling is preserved through a registered
/// prelude alias, not by weakening the element name.
pub fn plugin_element_name(package: &str, element: &str) -> String {
    format!("{package}::{element}")
}

/// Returns the legacy bare alias for a package whose id equals the local
/// element name (`shader::shader` -> `shader`). Qualified names remain
/// canonical in the Leaf tree.
pub fn plugin_legacy_alias(package: &str, element: &str) -> Option<String> {
    (package == element).then(|| element.to_owned())
}

fn element_schema(
    element_name: &str,
    element: &ElementDecl,
    package: &str,
) -> Result<ElementSchema, String> {
    let kind = match element.kind.as_deref() {
        None => {
            if element.block {
                ShapingKind::Block
            } else {
                ShapingKind::Inline
            }
        }
        Some("inline") => ShapingKind::Inline,
        Some("block") => ShapingKind::Block,
        Some("separator") => ShapingKind::Separator,
        Some(other) => {
            return Err(format!(
                "plugin `{package}` element `{element_name}` has unsupported kind `{other}`"
            ));
        }
    };
    let body_mode = match element.body_mode.as_deref() {
        None | Some("flow") => BodyMode::Flow,
        Some("inline") => BodyMode::Inline,
        Some("cells") => BodyMode::Cells,
        Some("none") => BodyMode::None,
        Some(other) => {
            return Err(format!(
                "plugin `{package}` element `{element_name}` has unsupported body-mode `{other}`"
            ));
        }
    };
    let role = match element.role.as_deref() {
        None | Some("none") => ShapingRole::None,
        Some("heading") => ShapingRole::Heading,
        Some("item") => ShapingRole::Item,
        Some(other) => {
            return Err(format!(
                "plugin `{package}` element `{element_name}` has unsupported role `{other}`"
            ));
        }
    };
    Ok(ElementSchema::new(
        ElementName::parse(element_name),
        kind,
        body_mode,
        role,
    ))
}

fn element_signature(element: &ElementDecl) -> Result<FunctionSignature, String> {
    let mut parameters = Vec::new();
    for param in &element.parameters {
        let ty = parse_type(&param.ty)?;
        // Optional parameters without an explicit value still default to
        // `none`, matching the built-in signature convention.
        let default = match &param.default {
            Some(value) => Some(json_default(value)?),
            None if matches!(ty, Type::Optional(_)) => Some(DefaultValue::None),
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
    store: Store<WasmStoreState>,
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
    owner: FunctionOwner,
}

impl Function for WasmFunction {
    fn name(&self) -> &str {
        &self.element_name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn owner(&self) -> FunctionOwner {
        self.owner.clone()
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

        let mut runtime = self.runtime.lock().map_err(|_| {
            vec![EvalDiagnostic {
                message: "plugin runtime lock poisoned".into(),
                range: input.range,
            }]
        })?;

        const INPUT_OFFSET: usize = 1024;
        let WasmRuntime {
            store,
            memory,
            evaluate,
            ..
        } = &mut *runtime;
        store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
            vec![EvalDiagnostic {
                message: format!("cannot reset wasm plugin fuel: {error}"),
                range: input.range,
            }]
        })?;
        if memory.data_size(&mut *store) < INPUT_OFFSET + request.len() {
            return Err(vec![EvalDiagnostic {
                message: "plugin request does not fit in wasm memory".into(),
                range: input.range,
            }]);
        }
        memory
            .write(&mut *store, INPUT_OFFSET, &request)
            .map_err(|error| {
                vec![EvalDiagnostic {
                    message: format!("cannot write wasm memory: {error}"),
                    range: input.range,
                }]
            })?;

        let mut results = [Val::I32(0)];
        evaluate
            .call(
                &mut *store,
                &[
                    Val::I32(INPUT_OFFSET as i32),
                    Val::I32(request.len() as i32),
                ],
                &mut results,
            )
            .map_err(|error| {
                let message = if error.to_string().contains("fuel") {
                    "wasm plugin exceeded its fuel budget".to_owned()
                } else {
                    format!("wasm plugin error: {error}")
                };
                vec![EvalDiagnostic {
                    message,
                    range: input.range,
                }]
            })?;
        let Val::I32(response_ptr) = results[0] else {
            return Err(vec![EvalDiagnostic {
                message: "wasm plugin returned non-i32".into(),
                range: input.range,
            }]);
        };

        let mut response = [0u8; 9];
        memory
            .read(&mut *store, response_ptr as usize, &mut response)
            .map_err(|error| {
                vec![EvalDiagnostic {
                    message: format!("cannot read wasm response: {error}"),
                    range: input.range,
                }]
            })?;
        if response[0] != 1 {
            return Err(vec![EvalDiagnostic {
                message: "wasm plugin returned ok=false".into(),
                range: input.range,
            }]);
        }
        let response_width = i32::from_le_bytes(response[1..5].try_into().unwrap());
        let response_height = i32::from_le_bytes(response[5..9].try_into().unwrap());

        let body = match self.signature.trailing_content.as_deref() {
            Some(name) => input.arguments.take_content(name),
            None => Content::new(),
        };
        let mut fields = BTreeMap::new();
        for param in &self.signature.parameters {
            if param.ty == Type::Content {
                continue;
            }
            if let Some(value) = input.arguments.get(&param.name) {
                fields.insert(param.name.clone(), value_to_json(value));
            }
        }
        if self
            .signature
            .parameters
            .iter()
            .any(|parameter| parameter.name == "width")
        {
            fields.insert(
                "width".to_string(),
                serde_json::Value::Number(response_width.into()),
            );
        }
        if self
            .signature
            .parameters
            .iter()
            .any(|parameter| parameter.name == "height")
        {
            fields.insert(
                "height".to_string(),
                serde_json::Value::Number(response_height.into()),
            );
        }
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

enum SemanticRuntime {
    Core(Arc<Mutex<WasmRuntime>>),
    Component(Arc<Mutex<ComponentRuntime>>),
    ComponentHost(Arc<Mutex<ComponentHostRuntime>>),
}

struct ComponentRuntime {
    store: Store<()>,
    bindings: wit_bindings_semantic::Semantic,
}

fn load_component_runtime(
    package: &str,
    wasm_path: &Path,
    wasm_bytes: &[u8],
) -> Result<ComponentRuntime, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("cannot create component engine for `{package}`: {error}"))?;
    let component = Component::new(&engine, wasm_bytes)
        .map_err(|error| format!("invalid component module {}: {error}", wasm_path.display()))?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, ());
    store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
        format!(
            "cannot set component instantiation fuel for {}: {error}",
            wasm_path.display()
        )
    })?;
    let bindings = wit_bindings_semantic::Semantic::instantiate(&mut store, &component, &linker)
        .map_err(|error| {
            format!(
                "cannot instantiate component {}: {error}",
                wasm_path.display()
            )
        })?;
    Ok(ComponentRuntime { store, bindings })
}

struct ComponentHostState {
    registry: Arc<Mutex<FunctionRegistry>>,
    owner: FunctionOwner,
}

impl wit_bindings::notist::plugin::host::Host for ComponentHostState {
    fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>, String> {
        let call = wire::decode_call(&request)?;
        let call = wire::wire_call_to_legacy(&call)?;
        let calls = notist_eval::CallContent {
            nodes: vec![notist_eval::CallNode::Call(call)],
        };
        let registry = self
            .registry
            .lock()
            .map_err(|_| "shared plugin registry lock poisoned".to_owned())?;
        let owner = self.owner.clone();
        let content =
            notist_eval::reduce_content_as(&calls, &registry, &owner).map_err(|diagnostics| {
                diagnostics
                    .into_iter()
                    .map(|diagnostic| diagnostic.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
        let nodes = wire::content_to_wire_nodes(&content)?;
        wire::encode_nodes(&nodes)
    }
}

struct ComponentHostRuntime {
    store: Store<ComponentHostState>,
    bindings: wit_bindings::SemanticHost,
}

fn load_component_host_runtime(
    package: &str,
    wasm_path: &Path,
    wasm_bytes: &[u8],
    shared_registry: Arc<Mutex<FunctionRegistry>>,
) -> Result<ComponentHostRuntime, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("cannot create component engine for `{package}`: {error}"))?;
    let component = Component::new(&engine, wasm_bytes)
        .map_err(|error| format!("invalid component module {}: {error}", wasm_path.display()))?;
    let mut linker = Linker::new(&engine);
    wit_bindings::SemanticHost::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
        .map_err(|error| format!("cannot link component host imports: {error}"))?;
    let mut store = Store::new(
        &engine,
        ComponentHostState {
            registry: shared_registry,
            owner: FunctionOwner::Plugin(package.to_owned()),
        },
    );
    store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
        format!(
            "cannot set component host instantiation fuel for {}: {error}",
            wasm_path.display()
        )
    })?;
    let bindings = wit_bindings::SemanticHost::instantiate(&mut store, &component, &linker)
        .map_err(|error| {
            format!(
                "cannot instantiate component {}: {error}",
                wasm_path.display()
            )
        })?;
    Ok(ComponentHostRuntime { store, bindings })
}

struct ComponentHostFunction {
    element_name: String,
    signature: FunctionSignature,
    runtime: Arc<Mutex<ComponentHostRuntime>>,
    owner: FunctionOwner,
}

impl Function for ComponentHostFunction {
    fn name(&self) -> &str {
        &self.element_name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn owner(&self) -> FunctionOwner {
        self.owner.clone()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let trailing = self.signature.trailing_content.clone();
        let body = match trailing.as_deref() {
            Some(name) => Some(input.arguments.take_content(name)),
            None => None,
        };
        let body = body
            .map(|content| wire::content_to_wire_nodes(&content))
            .transpose()
            .map_err(|message| {
                vec![EvalDiagnostic {
                    message,
                    range: input.range,
                }]
            })?;
        let mut arguments = Vec::new();
        for (name, value) in input.arguments.iter() {
            if Some(name) == trailing.as_deref() {
                continue;
            }
            arguments.push(wire::WireArgument {
                name: name.to_owned(),
                value: wire::eval_value_to_wire(value).map_err(|message| {
                    vec![EvalDiagnostic {
                        message,
                        range: input.range,
                    }]
                })?,
            });
        }
        let request = wire::encode_call(&wire::WireCall {
            name: self.element_name.clone(),
            arguments,
            body,
        })
        .map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;

        let mut runtime = self.runtime.lock().map_err(|_| {
            vec![EvalDiagnostic {
                message: "component host runtime lock poisoned".into(),
                range: input.range,
            }]
        })?;
        let ComponentHostRuntime { store, bindings } = &mut *runtime;
        store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
            vec![EvalDiagnostic {
                message: format!("cannot reset component host fuel: {error}"),
                range: input.range,
            }]
        })?;
        let response = bindings
            .call_evaluate(&mut *store, &request)
            .map_err(|error| {
                vec![EvalDiagnostic {
                    message: if error.to_string().contains("fuel") {
                        "wasm component exceeded its fuel budget".into()
                    } else {
                        format!("wasm component error: {error}")
                    },
                    range: input.range,
                }]
            })?
            .map_err(|message| {
                vec![EvalDiagnostic {
                    message: format!("wasm component returned error: {message}"),
                    range: input.range,
                }]
            })?;
        if response.len() > MAX_COMPONENT_RESPONSE_BYTES {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "wasm component response exceeded the limit of {MAX_COMPONENT_RESPONSE_BYTES} bytes"
                ),
                range: input.range,
            }]);
        }
        let nodes = wire::decode_nodes(&response).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        wire::validate_nodes(&nodes, MAX_COMPONENT_RESPONSE_NODES).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        let calls = wire::wire_nodes_to_call_content(&nodes).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        Ok(FunctionOutput::calls(calls))
    }
}

struct ComponentFunction {
    element_name: String,
    signature: FunctionSignature,
    runtime: Arc<Mutex<ComponentRuntime>>,
    owner: FunctionOwner,
}

impl Function for ComponentFunction {
    fn name(&self) -> &str {
        &self.element_name
    }

    fn signature(&self) -> FunctionSignature {
        self.signature.clone()
    }

    fn owner(&self) -> FunctionOwner {
        self.owner.clone()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let trailing = self.signature.trailing_content.clone();
        let body = match trailing.as_deref() {
            Some(name) => Some(input.arguments.take_content(name)),
            None => None,
        };
        let body = body
            .map(|content| wire::content_to_wire_nodes(&content))
            .transpose()
            .map_err(|message| {
                vec![EvalDiagnostic {
                    message,
                    range: input.range,
                }]
            })?;
        let mut arguments = Vec::new();
        for (name, value) in input.arguments.iter() {
            if Some(name) == trailing.as_deref() {
                continue;
            }
            arguments.push(wire::WireArgument {
                name: name.to_owned(),
                value: wire::eval_value_to_wire(value).map_err(|message| {
                    vec![EvalDiagnostic {
                        message,
                        range: input.range,
                    }]
                })?,
            });
        }
        let request = wire::encode_call(&wire::WireCall {
            name: self.element_name.clone(),
            arguments,
            body,
        })
        .map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;

        let mut runtime = self.runtime.lock().map_err(|_| {
            vec![EvalDiagnostic {
                message: "component runtime lock poisoned".into(),
                range: input.range,
            }]
        })?;
        let ComponentRuntime { store, bindings } = &mut *runtime;
        store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
            vec![EvalDiagnostic {
                message: format!("cannot reset component fuel: {error}"),
                range: input.range,
            }]
        })?;
        let response = bindings
            .call_evaluate(&mut *store, &request)
            .map_err(|error| {
                vec![EvalDiagnostic {
                    message: if error.to_string().contains("fuel") {
                        "wasm component exceeded its fuel budget".into()
                    } else {
                        format!("wasm component error: {error}")
                    },
                    range: input.range,
                }]
            })?
            .map_err(|message| {
                vec![EvalDiagnostic {
                    message: format!("wasm component returned error: {message}"),
                    range: input.range,
                }]
            })?;
        if response.len() > MAX_COMPONENT_RESPONSE_BYTES {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "wasm component response exceeded the limit of {MAX_COMPONENT_RESPONSE_BYTES} bytes"
                ),
                range: input.range,
            }]);
        }
        let nodes = wire::decode_nodes(&response).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        wire::validate_nodes(&nodes, MAX_COMPONENT_RESPONSE_NODES).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        let calls = wire::wire_nodes_to_call_content(&nodes).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        Ok(FunctionOutput::calls(calls))
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
        notist_eval::Value::Float(value) => serde_json::Number::from_f64(*value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
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
///
/// The canonical name is always `package::element`. For legacy packages whose
/// id equals the element name, the bare name is also registered as an alias so
/// existing documents such as `#shader(...)` keep resolving.
pub fn register_loaded(
    registry: &mut FunctionRegistry,
    plugins: &[LoadedPlugin],
) -> Result<(), RegistryError> {
    for plugin in plugins {
        let principal = Principal::Plugin(plugin.id.clone());
        for capability in &plugin.capabilities {
            registry.allow(principal.clone(), capability.clone());
        }
        for function in &plugin.functions {
            registry.register_arc(Arc::clone(function))?;
            let Some((package, element)) = function.name().split_once("::") else {
                continue;
            };
            if let Some(alias) = plugin_legacy_alias(package, element) {
                registry.register_alias(alias, function.name())?;
            }
        }
    }
    Ok(())
}

/// Registers the shaping schemas contributed by loaded plugin packages.
pub fn register_loaded_shaping(registry: &mut ShapingRegistry, plugins: &[LoadedPlugin]) {
    for plugin in plugins {
        for schema in &plugin.elements {
            registry.insert(schema.clone());
        }
    }
}

pub mod wire;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use notist_eval::{Evaluator, FunctionRegistry};

    use super::*;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_package_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "notist-plugin-host-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(dir: &std::path::Path, manifest: &str) {
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
    }

    #[test]
    fn declarative_package_without_wasm_registers_and_evaluates() {
        let dir = temp_package_dir();
        write_manifest(
            &dir,
            r#"{
                "package": "demo",
                "version": "0.1.0",
                "api-version": "0.1",
                "interfaces": {
                    "semantic": {
                        "elements": [{
                            "name": "box",
                            "version": 1,
                            "block": true,
                            "parameters": [
                                { "name": "source", "ty": "String" },
                                { "name": "width", "ty": "Int", "default": 800 }
                            ],
                            "trailing-content": "body",
                            "body-mode": "flow",
                            "role": "none"
                        }]
                    }
                }
            }"#,
        );

        let plugin = load_package(&dir).unwrap();
        assert_eq!(plugin.id, "demo");
        assert_eq!(plugin.functions.len(), 1);
        assert_eq!(plugin.functions[0].name(), "demo::box");
        assert_eq!(plugin.elements.len(), 1);
        assert_eq!(plugin.elements[0].name, ElementName::plugin("demo", "box"));

        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::box(source: \"wgsl\")[Hi]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom {
            name, fields, body, ..
        } = &evaluation.content.elements[0].element
        else {
            panic!("expected custom element")
        };
        assert_eq!(name, "demo::box");
        assert!(fields.iter().any(|field| field.name == "width"
            && matches!(field.value, ElementValue::Int(800))));
        assert!(matches!(&body.elements[0].element, Element::Text(text) if text == "Hi"));
    }

    #[test]
    fn declarative_bodyless_element_does_not_panic() {
        let dir = temp_package_dir();
        write_manifest(
            &dir,
            r#"{
                "package": "demo",
                "version": "0.1.0",
                "api-version": "0.1",
                "interfaces": {
                    "semantic": {
                        "elements": [{
                            "name": "badge",
                            "version": 1,
                            "block": false,
                            "parameters": [{ "name": "label", "ty": "String" }]
                        }]
                    }
                }
            }"#,
        );

        let plugin = load_package(&dir).unwrap();
        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#demo::badge(label: \"x\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom { body, .. } = &evaluation.content.elements[0].element else {
            panic!("expected custom element")
        };
        assert!(body.elements.is_empty());
    }

    #[test]
    fn core_manifest_matches_the_builtin_function_surface() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/core")
            .canonicalize()
            .unwrap();
        let manifest_text = std::fs::read_to_string(package_dir.join("plugin.json")).unwrap();
        let manifest: PluginManifest = serde_json::from_str(&manifest_text).unwrap();
        let semantic = manifest.interfaces.semantic.as_ref().unwrap();
        let mut declared = semantic
            .elements
            .iter()
            .map(|element| element.name.clone())
            .collect::<Vec<_>>();
        declared.sort();

        let text_signature = FunctionSignature {
            parameters: vec![Parameter {
                name: "text".into(),
                ty: Type::String,
                default: None,
            }],
            trailing_content: None,
            result: Type::Content,
        };
        let mut expected = vec![
            ("text", text_signature),
            ("parbreak", notist_model::empty_content_signature()),
        ];
        expected.extend(notist_model::builtin_signatures());
        let mut expected_names = expected
            .iter()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();
        expected_names.sort();
        assert_eq!(declared, expected_names);

        for (name, expected_signature) in expected {
            let element = semantic
                .elements
                .iter()
                .find(|element| element.name == name)
                .unwrap();
            assert_eq!(
                element_signature(element).unwrap(),
                expected_signature,
                "core manifest signature mismatch for `{name}`"
            );
        }
    }

    #[test]
    fn plugin_shaping_schema_applies_in_the_stream_pipeline() {
        let dir = temp_package_dir();
        write_manifest(
            &dir,
            r#"{
                "package": "demo",
                "version": "0.1.0",
                "api-version": "0.1",
                "interfaces": {
                    "semantic": {
                        "elements": [{
                            "name": "box",
                            "version": 1,
                            "block": true,
                            "parameters": [{ "name": "source", "ty": "String" }],
                            "trailing-content": "body",
                            "body-mode": "flow",
                            "role": "none"
                        }]
                    }
                }
            }"#,
        );
        let plugins = [load_package(&dir).unwrap()];
        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &plugins).unwrap();
        let mut shaping = ShapingRegistry::new();
        register_loaded_shaping(&mut shaping, &plugins);

        let evaluation = Evaluator::new(registry)
            .evaluate_stream_with_shaping("#demo::box(source: \"x\")[first\n\nsecond]", &shaping);
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert_eq!(
            evaluation.tree.roots[0].instance.name,
            ElementName::plugin("demo", "box")
        );
        assert_eq!(evaluation.tree.roots[0].instance.body.len(), 2);
        assert!(
            evaluation.tree.roots[0]
                .instance
                .body
                .iter()
                .all(|node| node.instance.is_core("paragraph"))
        );
    }

    #[test]
    fn wit_component_package_evaluates_through_the_bytes_abi() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "component-echo");
        assert_eq!(plugin.functions[0].name(), "component-echo::echo");

        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation =
            Evaluator::new(registry).evaluate("#component-echo::echo(message: \"hi\")");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom { name, fields, .. } = &evaluation.content.elements[0].element else {
            panic!(
                "expected custom element, got {:?}",
                evaluation.content.elements
            )
        };
        assert_eq!(name, "demo::echo");
        assert!(fields.iter().any(|field| field.name == "message"
            && matches!(&field.value, ElementValue::String(value) if value == "hello from component")));
    }

    #[test]
    fn vault_site_grants_intersect_plugin_capabilities() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-host-call")
            .canonicalize()
            .unwrap();
        let granted =
            load_package_with_grants(&package_dir, Some(&["core::text".to_owned()])).unwrap();
        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[granted]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#component-host-call::passthrough()");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );

        let denied = load_package_with_grants(&package_dir, Some(&[])).unwrap();
        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[denied]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#component-host-call::passthrough()");
        assert!(
            evaluation.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("not allowed to call `core::text`")),
            "{:?}",
            evaluation.diagnostics
        );
    }

    #[test]
    fn component_host_call_enforces_declared_capabilities() {
        let source_package = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-host-call")
            .canonicalize()
            .unwrap();
        let dir = temp_package_dir();
        std::fs::copy(
            source_package.join("semantic.wasm"),
            dir.join("semantic.wasm"),
        )
        .unwrap();
        write_manifest(
            &dir,
            r#"{
                "package": "component-host-call-denied",
                "version": "0.1.0",
                "api-version": "0.1",
                "interfaces": {
                    "semantic": {
                        "elements": [{
                            "name": "passthrough",
                            "version": 1,
                            "block": false,
                            "parameters": []
                        }]
                    }
                },
                "capabilities": { "request": [] },
                "wasm": {
                    "module": "semantic.wasm",
                    "component": true,
                    "host-call": true
                }
            }"#,
        );

        let plugin = load_package(&dir).unwrap();
        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation =
            Evaluator::new(registry).evaluate("#component-host-call-denied::passthrough()");
        assert!(
            evaluation.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("not allowed to call `core::text`")),
            "{:?}",
            evaluation.diagnostics
        );
    }

    #[test]
    fn component_host_call_reduces_through_the_plugin_registry() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-host-call")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "component-host-call");
        assert!(plugin.shared_registry.is_some());

        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#component-host-call::passthrough()");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert!(matches!(
            &evaluation.content.elements[0].element,
            Element::Text(text) if text == "hello from host.call"
        ));
    }

    #[test]
    fn shader_package_still_loads_through_v0_wasm_abi() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/shader")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "shader");
        assert_eq!(plugin.functions.len(), 1);
        assert_eq!(plugin.functions[0].name(), "shader::shader");
        assert_eq!(plugin.html_contributions.len(), 1);
        assert_eq!(plugin.html_contributions[0].element, "shader");
        assert!(plugin.html_contributions[0].trusted);
        let component = plugin.html_contributions[0]
            .web_component
            .as_ref()
            .expect("shader declares a web component");
        assert_eq!(component.tag, "notist-shader");
        assert_eq!(component.module, "assets/shader.js");

        let mut registry = FunctionRegistry::with_builtins();
        register_loaded(&mut registry, &[plugin]).unwrap();
        assert!(registry.get("shader").is_some(), "legacy bare alias");
        assert!(registry.get("shader::shader").is_some(), "qualified name");
        let evaluation = Evaluator::new(registry).evaluate(
            "#shader(source: \"fn mainImage(fragCoord: vec2<f32>) -> vec4<f32> { return vec4<f32>(fragCoord, 0.0, 1.0); }\", width: 320, height: 200)[fallback]",
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Element::Custom { fields, .. } = &evaluation.content.elements[0].element else {
            panic!("expected custom element")
        };
        assert!(fields.iter().any(|field| field.name == "width"
            && matches!(field.value, ElementValue::Int(320))));
        assert!(
            fields.iter().any(
                |field| field.name == "height" && matches!(field.value, ElementValue::Int(200))
            )
        );
    }
}

#[allow(dead_code, clippy::all)]
mod wit_bindings {
    wasmtime::component::bindgen!({ path: "wit/notist-plugin.wit", world: "semantic-host" });
}

#[allow(dead_code, clippy::all)]
mod wit_bindings_semantic {
    wasmtime::component::bindgen!({ path: "wit/notist-plugin.wit", world: "semantic" });
}
