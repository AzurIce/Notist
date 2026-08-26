//! Runtime Wasm component plugin host for Notist.
//!
//! This loader reads `Notist.toml`, resolves plugin package directories,
//! reads the envelope `plugin.json`, and loads WebAssembly components with
//! Wasmtime.
//!
//! Component packages are self-describing: after instantiation the host calls
//! `init()` exactly once to collect shared `notist-model` declarations,
//! validates them atomically, and registers the resulting functions and
//! schemas. `plugin.json` only describes the package envelope (identity, Wasm
//! module, and render assets).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use notist_eval::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOwner, FunctionRegistry,
    FunctionSignature, PluginContribution, RegistryError, ShapingRegistry, Type, Value,
};
use notist_model::{
    BodyMode, DefaultValue, ElementName, ElementSchema, Parameter, PluginElementDecl, ShapingKind,
    ShapingRole,
};
#[cfg(test)]
use notist_plugin_core as core_plugin;
use serde::Deserialize;
use wasmtime::component::{Component as WasmComponent, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p2};

/// A plugin entry in `Notist.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct VaultPluginConfig {
    /// Path to the plugin package directory, relative to the vault root.
    pub path: Option<String>,
    /// A future registry package name.
    pub package: Option<String>,
}

/// Site-level presentation configuration under `[site]` in `Notist.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
struct VaultSiteConfig {
    /// Extra stylesheets as vault-root-relative `/`-separated paths. The CLI
    /// site layer copies each file into `_notist/styles/` and links it from
    /// every page head after the built-in `_notist/style.css`.
    #[serde(default)]
    styles: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VaultConfig {
    #[serde(default)]
    plugins: BTreeMap<String, VaultPluginConfig>,
    #[serde(default)]
    site: VaultSiteConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginManifest {
    pub package: String,
    pub version: String,
    #[serde(rename = "api-version")]
    pub api_version: String,
    /// Built-in or external implementation source, when the envelope declares one.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub wasm: Option<WasmDecl>,
    /// Envelope-style projection contributions.
    #[serde(default)]
    pub render: Option<ManifestRender>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WasmDecl {
    pub module: String,
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
    /// Eval-side contributions with runtime handlers. Data-only
    /// declarations appear in `signatures` but not here.
    pub functions: Vec<Arc<dyn Function>>,
    /// Signatures for every declared element (computed or data-only),
    /// qualified by package namespace.
    pub signatures: Vec<(String, FunctionSignature)>,
    /// Shaping schemas for each declared element.
    pub elements: Vec<ElementSchema>,
    /// HTML renderer contributions declared by the manifest.
    pub html_contributions: Vec<HtmlContribution>,
}

impl LoadedPlugin {
    /// Projects the loaded package onto the eval contribution contract.
    pub fn contribution(&self) -> PluginContribution {
        let mut contribution = PluginContribution::new(self.id.clone());
        contribution.functions = self.functions.clone();
        contribution.signatures = self.signatures.clone();
        contribution.elements = self.elements.clone();
        contribution.aliases = self
            .functions
            .iter()
            .filter_map(|function| {
                let (package, element) = function.name().split_once("::")?;
                plugin_legacy_alias(package, element)
                    .map(|alias| (alias, function.name().to_owned()))
            })
            .collect();
        contribution
    }
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

/// Resolves a plugin package path to a loadable directory.
///
/// Directory packages are returned unchanged. Zip packages are extracted into
/// a deterministic cache directory under the system temp dir; extraction is
/// skipped when the cached copy is at least as new as the zip file.
pub fn resolve_package_dir(package_path: &Path) -> Result<PathBuf, String> {
    if package_path.is_dir() {
        return Ok(package_path.to_path_buf());
    }
    if package_path.extension().and_then(|ext| ext.to_str()) != Some("zip") {
        return Err(format!(
            "plugin package `{}` is neither a directory nor a zip file",
            package_path.display()
        ));
    }
    let source = package_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", package_path.display()))?;
    let source_mtime = source
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("cannot read {} metadata: {error}", source.display()))?;
    let cache_root = std::env::temp_dir().join("notist-plugin-packages");
    let cache_dir = cache_root.join(format!(
        "{:016x}",
        fnv1a(source.to_string_lossy().as_bytes())
    ));
    let marker = cache_dir.join(".source-mtime");
    let fresh = cache_dir.join("plugin.json").is_file()
        && std::fs::read_to_string(&marker).ok().as_deref() == Some(&format!("{source_mtime:?}"));
    if fresh {
        return Ok(cache_dir);
    }

    if cache_dir.exists() {
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("cannot create plugin cache: {error}"))?;
    let file = std::fs::File::open(&source)
        .map_err(|error| format!("cannot open {}: {error}", source.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("invalid plugin zip {}: {error}", source.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("invalid plugin zip entry: {error}"))?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let relative = name.as_path();
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(format!("unsafe zip entry `{}`", name.display()));
        }
        let target = cache_dir.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            let mut output = std::fs::File::create(&target)
                .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
            std::io::copy(&mut entry, &mut output)
                .map_err(|error| format!("cannot extract {}: {error}", target.display()))?;
        }
    }
    std::fs::write(&marker, format!("{source_mtime:?}"))
        .map_err(|error| format!("cannot write plugin cache marker: {error}"))?;
    Ok(cache_dir)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
        let plugin = load_package(&package_dir)?;
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
        let manifest = read_manifest(&resolve_package_dir(&package_dir)?)?;
        let contributions = manifest
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

/// Returns the extra site stylesheets declared under `[site] styles`.
///
/// Entries are returned as normalized vault-root-relative `/`-separated paths
/// in declaration order with duplicates removed. Validation rejects absolute
/// paths, backslash separators, and any path that escapes the vault root.
pub fn site_styles(toml_text: Option<&str>) -> Result<Vec<String>, String> {
    let Some(toml_text) = toml_text else {
        return Ok(Vec::new());
    };
    let config: VaultConfig =
        toml::from_str(toml_text).map_err(|error| format!("invalid Notist.toml: {error}"))?;
    let mut styles = Vec::new();
    for style in config.site.styles {
        let style = validate_site_style(&style)?;
        if !styles.contains(&style) {
            styles.push(style);
        }
    }
    Ok(styles)
}

/// Validates one `[site] styles` entry and returns its normalized form.
fn validate_site_style(style: &str) -> Result<String, String> {
    if style.is_empty() {
        return Err("`[site] styles` entry must not be empty".into());
    }
    if style.contains('\\') {
        return Err(format!(
            "`[site] styles` entry `{style}` must use `/` separators"
        ));
    }
    let mut normalized = String::new();
    for component in Path::new(style).components() {
        match component {
            Component::Normal(segment) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&segment.to_string_lossy());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "`[site] styles` entry `{style}` must be relative to the vault root"
                ));
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(format!(
                    "`[site] styles` entry `{style}` must stay inside the vault root"
                ));
            }
        }
    }
    if normalized.is_empty() {
        return Err("`[site] styles` entry must name a file".into());
    }
    if Path::new(&normalized)
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("css")
    {
        return Err(format!(
            "`[site] styles` entry `{style}` must name a `.css` file"
        ));
    }
    Ok(normalized)
}

/// Loads one self-describing Wasm component package directory.
pub fn load_package(package_dir: &Path) -> Result<LoadedPlugin, String> {
    let package_dir = resolve_package_dir(package_dir)?;
    let manifest = read_manifest(&package_dir)?;
    let wasm = manifest.wasm.as_ref().ok_or_else(|| {
        format!(
            "plugin `{}` declares no wasm module; packages must ship a self-describing component",
            manifest.package
        )
    })?;
    let wasm_path = package_dir.join(&wasm.module);
    let wasm_bytes = std::fs::read(&wasm_path)
        .map_err(|error| format!("cannot read {}: {error}", wasm_path.display()))?;
    let (runtime, declarations) =
        load_component_runtime(&manifest.package, &wasm_path, &wasm_bytes)?;
    let runtime = Arc::new(Mutex::new(runtime));

    let mut functions = Vec::new();
    let mut signatures = Vec::new();
    let mut elements = Vec::new();
    for element in &declarations {
        let signature = element_signature(element)?;
        let element_name = plugin_element_name(&manifest.package, &element.name);
        signatures.push((element_name.clone(), signature.clone()));
        if !element.computed {
            // Data-only declaration: the document call IS the final leaf.
            // Register schema + signature only; no dispatch entry.
            elements.push(element_schema(&element_name, element, &manifest.package)?);
            continue;
        }
        let function: Arc<dyn Function> = Arc::new(ComponentFunction {
            element_name: element_name.clone(),
            signature,
            runtime: Arc::clone(&runtime),
            owner: FunctionOwner::Package(manifest.package.clone()),
        });
        functions.push(function);
        elements.push(element_schema(&element_name, element, &manifest.package)?);
    }

    let html_contributions = manifest
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
        signatures,
        elements,
        html_contributions,
    })
}

/// Fuel granted to one plugin dispatch. With `consume_fuel` enabled this
/// bounds even non-terminating Wasm loops without adding a timer thread.
///
/// Sized so a cold first dispatch can pay one-time guest lazy
/// initialization (regex tables, parser tables): measured cold-start parse
/// of a flowchart through the mermaid plugin costs ~21M fuel, warm parses
/// stay under ~4M.
const WASM_FUEL_PER_CALL: u64 = 50_000_000;

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

/// Returns the qualified call-site and element name for a manifest element.
///
/// All plugin elements are namespaced by their package (`shader::shader`,
/// `demo::box`). The original bare `shader` spelling is preserved through a
/// registered prelude alias, not by weakening the element name.
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
    element: &PluginElementDecl,
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

fn element_signature(element: &PluginElementDecl) -> Result<FunctionSignature, String> {
    let mut parameters = Vec::new();
    for param in &element.parameters {
        let ty = parse_type(&param.ty)?;
        // Optional parameters without an explicit value still default to
        // `none`, matching the built-in signature convention.
        let default = param
            .default
            .clone()
            .or_else(|| matches!(ty, Type::Optional(_)).then_some(DefaultValue::None));
        if let Some(default) = &default
            && !ty.accepts(&default.ty())
        {
            return Err(format!(
                "plugin element `{}` parameter `{}` has default type `{}` but declares `{ty}`",
                element.name,
                param.name,
                default.ty()
            ));
        }
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

struct ComponentRuntime {
    store: Store<ComponentStoreState>,
    bindings: wit_bindings_semantic::Plugin,
}

/// Store state for a plain `plugin` world component: resource limits plus the
/// empty WASI context required by SDK-generated guests.
struct ComponentStoreState {
    limits: WasmStoreState,
    table: ResourceTable,
    wasi: WasiCtx,
}

impl WasiView for ComponentStoreState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl ResourceLimiter for ComponentStoreState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.memory_growing(current, desired, maximum)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }
}

/// Calls guest `init` exactly once after instantiation, inside the same fuel
/// budget as `evaluate`, and decodes its shared declaration payload.
fn run_component_init(
    package: &str,
    wasm_path: &Path,
    bindings: &wit_bindings_semantic::Plugin,
    store: &mut Store<ComponentStoreState>,
) -> Result<Vec<PluginElementDecl>, String> {
    store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
        format!(
            "cannot set component init fuel for {}: {error}",
            wasm_path.display()
        )
    })?;
    let payload = bindings
        .call_init(store)
        .map_err(|error| component_init_error(package, wasm_path, error))?
        .map_err(|message| {
            format!("plugin `{package}`: component init returned error: {message}")
        })?;
    notist_model::wire::decode_declarations(&payload)
        .map_err(|message| format!("plugin `{package}` returned invalid init payload: {message}"))
}

fn component_init_error(package: &str, wasm_path: &Path, error: wasmtime::Error) -> String {
    let reason = if error.to_string().contains("fuel") {
        "component init exceeded its fuel budget"
    } else {
        "component init failed"
    };
    format!(
        "plugin `{package}`: {reason} ({}): {error}",
        wasm_path.display()
    )
}

/// Validates one guest-declared element name. Guests can never escape their
/// package namespace because the host prepends `{package}::`; rejecting empty
/// names and qualified spellings keeps the namespace rule mechanical.
fn validate_guest_element_name(package: &str, name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && !name.contains("::")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "plugin `{package}` declared invalid element name `{name}`"
        ))
    }
}

fn load_component_runtime(
    package: &str,
    wasm_path: &Path,
    wasm_bytes: &[u8],
) -> Result<(ComponentRuntime, Vec<PluginElementDecl>), String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|error| format!("cannot create component engine for `{package}`: {error}"))?;
    let component = WasmComponent::new(&engine, wasm_bytes)
        .map_err(|error| format!("invalid component module {}: {error}", wasm_path.display()))?;
    let mut linker = Linker::<ComponentStoreState>::new(&engine);
    p2::add_to_linker_sync(&mut linker)
        .map_err(|error| format!("cannot link component wasi imports: {error}"))?;
    let mut store = Store::new(
        &engine,
        ComponentStoreState {
            limits: WasmStoreState {
                max_memory: WASM_MAX_MEMORY_BYTES,
                max_table_elements: WASM_MAX_TABLE_ELEMENTS,
            },
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
        },
    );
    store.limiter(|state| state);
    store.set_fuel(WASM_FUEL_PER_CALL).map_err(|error| {
        format!(
            "cannot set component instantiation fuel for {}: {error}",
            wasm_path.display()
        )
    })?;
    let bindings = wit_bindings_semantic::Plugin::instantiate(&mut store, &component, &linker)
        .map_err(|error| {
            format!(
                "cannot instantiate component {}: {error}",
                wasm_path.display()
            )
        })?;
    let declarations = run_component_init(package, wasm_path, &bindings, &mut store)?;
    for declaration in &declarations {
        validate_guest_element_name(package, &declaration.name)?;
    }
    Ok((ComponentRuntime { store, bindings }, declarations))
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
    ) -> Result<Value, Vec<EvalDiagnostic>> {
        let request_node =
            wire::build_request_node(&self.element_name, &self.signature, &mut input).map_err(
                |message| {
                    vec![EvalDiagnostic {
                        message,
                        range: input.range,
                    }]
                },
            )?;
        let request = notist_model::wire::encode_forest(std::slice::from_ref(&request_node))
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
        let returned = wire::decode_response(&response, input.range)?;
        Ok(Value::Content(returned))
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
    let mut shaping = ShapingRegistry::new();
    register_loaded_contributions(registry, &mut shaping, plugins)
}

/// Atomically installs all loaded package contributions into eval registries.
pub fn register_loaded_contributions(
    registry: &mut FunctionRegistry,
    shaping: &mut ShapingRegistry,
    plugins: &[LoadedPlugin],
) -> Result<(), RegistryError> {
    let mut candidate_registry = registry.clone();
    let mut candidate_shaping = shaping.clone();
    for plugin in plugins {
        candidate_registry.register_contribution(&mut candidate_shaping, &plugin.contribution())?;
    }
    *registry = candidate_registry;
    *shaping = candidate_shaping;
    Ok(())
}

/// Registers the shaping schemas contributed by loaded plugin packages.
///
/// This compatibility entry point retains its historical replacement behavior;
/// new callers should use [`register_loaded_contributions`].
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

    use notist_eval::{Evaluator, FunctionOwner, FunctionRegistry};
    use notist_model::NodeValue;

    use super::*;

    fn core_registry() -> FunctionRegistry {
        core_plugin::registry().0
    }

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
    fn component_init_registration_builds_signatures_and_schemas() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "component-echo");
        // echo is computed (dispatch entry); note is data-only (signature +
        // schema only). Both come from guest `init` — the envelope manifest
        // carries no semantic block at all.
        assert_eq!(plugin.functions.len(), 1);
        assert_eq!(plugin.functions[0].name(), "component-echo::echo");
        assert_eq!(plugin.signatures.len(), 2);
        assert!(
            plugin
                .signatures
                .iter()
                .any(|(name, _)| name == "component-echo::note")
        );
        assert!(!plugin.signatures.iter().any(|(name, _)| {
            name == "component-echo::note"
                && plugin
                    .functions
                    .iter()
                    .any(|f| f.name() == "component-echo::note")
        }));
        let signature = plugin.functions[0].signature();
        assert_eq!(signature.trailing_content.as_deref(), Some("body"));
        let message = signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == "message")
            .expect("message parameter registered from init");
        assert!(matches!(
            message.default,
            Some(DefaultValue::String(ref value)) if value == "hello"
        ));
        assert_eq!(plugin.elements.len(), 2);
        let note = plugin
            .elements
            .iter()
            .find(|schema| schema.name == ElementName::plugin("component-echo", "note"))
            .expect("note schema registered");
        assert_eq!(note.body_mode, notist_model::BodyMode::Flow);
        assert!(
            plugin
                .elements
                .iter()
                .any(|schema| schema.name == ElementName::plugin("component-echo", "echo"))
        );
    }

    #[test]
    fn data_only_elements_stay_as_leaves() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        let mut shaping = ShapingRegistry::new();
        let mut registry = core_registry();
        register_loaded_contributions(&mut registry, &mut shaping, std::slice::from_ref(&plugin))
            .unwrap();
        assert_eq!(
            registry.get("component-echo::echo").unwrap().owner(),
            FunctionOwner::Package("component-echo".into())
        );
        let evaluation = Evaluator::new(registry)
            .evaluate_with_shaping("#component-echo::note(message: \"Hello\")[body]", &shaping);
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        // The call stayed unreduced: self-named leaf with raw bound args,
        // body shaped by the guest-declared `body-mode: flow` schema.
        let root = &evaluation.tree.roots[0];
        assert_eq!(root.name, "component-echo::note");
        assert!(root.args.iter().any(|(name, value)| name == "message"
            && matches!(value, notist_model::NodeValue::String(value) if value == "Hello")));
        assert!(root.children.iter().all(|node| node.is_core("paragraph")));
    }

    #[test]
    fn core_manifest_is_an_identity_only_builtin_envelope() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/core")
            .canonicalize()
            .unwrap();
        let manifest = read_manifest(&package_dir).unwrap();
        assert_eq!(manifest.package, "core");
        assert_eq!(manifest.source.as_deref(), Some("builtin"));
        assert!(manifest.wasm.is_none());
        assert!(manifest.render.is_none());
    }

    #[test]
    fn plugin_shaping_schema_applies_in_the_stream_pipeline() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let plugins = [load_package(&package_dir).unwrap()];
        let mut registry = core_registry();
        register_loaded(&mut registry, &plugins).unwrap();
        let (_, mut shaping) = core_plugin::registry();
        register_loaded_shaping(&mut shaping, &plugins);

        let evaluation = Evaluator::new(registry).evaluate_with_shaping(
            "#component-echo::echo(message: \"x\")[first\n\nsecond]",
            &shaping,
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        // Pass-through: the body flows back through the component and the
        // guest-declared `body-mode: flow` schema shapes it into paragraphs.
        assert_eq!(evaluation.tree.roots.len(), 2);
        assert!(
            evaluation
                .tree
                .roots
                .iter()
                .all(|node| node.is_core("paragraph"))
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

        let mut registry = core_registry();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation =
            Evaluator::new(registry).evaluate("#component-echo::echo(message: \"hi\")[body]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        // Pass-through contract: the trailing body comes back verbatim.
        let node = &evaluation.forest[0];
        assert!(node.is_core("text"));
        assert_eq!(node.get("text"), Some(&NodeValue::String("body".into())));
    }

    #[test]
    fn envelope_render_contributions_parse_from_manifest() {
        let echo_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let dir = temp_package_dir();
        std::fs::copy(echo_dir.join("semantic.wasm"), dir.join("semantic.wasm")).unwrap();
        write_manifest(
            &dir,
            r#"{
                "package": "card",
                "version": "0.1.0",
                "api-version": "0.1",
                "render": {
                    "html": {
                        "contributions": [{
                            "element": "echo",
                            "trusted": true,
                            "web-component": {
                                "tag": "notist-card",
                                "module": "assets/card.js"
                            }
                        }]
                    }
                },
                "wasm": { "module": "semantic.wasm" }
            }"#,
        );
        let plugin = load_package(&dir).unwrap();
        assert_eq!(plugin.html_contributions.len(), 1);
        assert!(plugin.html_contributions[0].trusted);
    }

    #[test]
    fn zip_package_is_extracted_and_loaded() {
        use std::io::Write as _;
        let echo_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/component-echo")
            .canonicalize()
            .unwrap();
        let dir = temp_package_dir();
        write_manifest(
            &dir,
            r#"{
                "package": "zip-demo",
                "version": "0.1.0",
                "api-version": "0.1",
                "wasm": { "module": "semantic.wasm" }
            }"#,
        );
        let zip_path = dir.join("package.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("plugin.json", options).unwrap();
        writer
            .write_all(std::fs::read(dir.join("plugin.json")).unwrap().as_slice())
            .unwrap();
        writer.start_file("semantic.wasm", options).unwrap();
        writer
            .write_all(
                std::fs::read(echo_dir.join("semantic.wasm"))
                    .unwrap()
                    .as_slice(),
            )
            .unwrap();
        writer.finish().unwrap();

        let resolved = resolve_package_dir(&zip_path).unwrap();
        assert_ne!(resolved, dir);
        assert!(resolved.join("plugin.json").is_file());
        // The envelope carries no semantic block; registration comes from
        // guest init even for extracted zip packages.
        let plugin = load_package(&zip_path).unwrap();
        assert_eq!(plugin.id, "zip-demo");
        assert_eq!(plugin.functions[0].name(), "zip-demo::echo");
        let mut registry = core_registry();
        register_loaded(&mut registry, &[plugin]).unwrap();
        let evaluation = Evaluator::new(registry).evaluate("#zip-demo::echo(message: \"m\")[]");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let node = &evaluation.forest[0];
        assert!(node.is_core("text"));
        assert_eq!(node.get("text"), Some(&NodeValue::String("m".into())));
    }

    #[test]
    fn shader_component_initializes_and_reduces_bare_and_qualified_calls() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/shader")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "shader");
        assert_eq!(plugin.functions.len(), 1);
        assert_eq!(plugin.functions[0].name(), "shader::shader");
        assert_eq!(plugin.signatures.len(), 2);
        assert!(
            plugin
                .signatures
                .iter()
                .any(|(name, _)| name == "shader::canvas")
        );
        assert_eq!(plugin.html_contributions.len(), 1);
        assert_eq!(plugin.html_contributions[0].element, "canvas");
        assert!(plugin.html_contributions[0].trusted);
        let component = plugin.html_contributions[0]
            .web_component
            .as_ref()
            .expect("shader declares a web component");
        assert_eq!(component.tag, "notist-shader");
        assert_eq!(component.module, "assets/shader.js");

        let mut registry = core_registry();
        let mut shaping = ShapingRegistry::new();
        register_loaded_contributions(&mut registry, &mut shaping, &[plugin]).unwrap();
        assert!(registry.get("shader").is_some(), "bare handler alias");
        assert!(
            registry.get("shader::shader").is_some(),
            "qualified handler"
        );
        let evaluator = Evaluator::new(registry);

        let bare = evaluator.evaluate_with_shaping(
            "#shader(source: \"shader-source\", width: 320, height: 200)[fallback]",
            &shaping,
        );
        assert!(bare.diagnostics.is_empty(), "{:?}", bare.diagnostics);
        let canvas = &bare.forest[0];
        assert_eq!(canvas.name, "shader::canvas");
        assert_eq!(
            canvas.get("source"),
            Some(&NodeValue::String("shader-source".into()))
        );
        assert_eq!(canvas.get("width"), Some(&NodeValue::Int(320)));
        assert_eq!(canvas.get("height"), Some(&NodeValue::Int(200)));
        assert!(!canvas.range.is_empty());
        assert!(canvas.children.iter().any(|child| child.is_core("text")
            && child.get("text") == Some(&NodeValue::String("fallback".into()))));

        let qualified =
            evaluator.evaluate_with_shaping("#shader::shader(source: \"qualified\")[]", &shaping);
        assert!(
            qualified.diagnostics.is_empty(),
            "{:?}",
            qualified.diagnostics
        );
        let canvas = &qualified.forest[0];
        assert_eq!(canvas.name, "shader::canvas");
        assert_eq!(canvas.get("width"), Some(&NodeValue::Int(800)));
        assert_eq!(canvas.get("height"), Some(&NodeValue::Int(600)));
    }

    #[test]
    fn mermaid_component_validates_source_and_reduces_to_data_leaf() {
        let package_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/mermaid")
            .canonicalize()
            .unwrap();
        let plugin = load_package(&package_dir).unwrap();
        assert_eq!(plugin.id, "mermaid");
        assert_eq!(plugin.functions.len(), 1);
        assert_eq!(plugin.functions[0].name(), "mermaid::mermaid");
        assert_eq!(plugin.signatures.len(), 2);
        assert!(
            plugin
                .signatures
                .iter()
                .any(|(name, _)| name == "mermaid::diagram")
        );
        assert_eq!(plugin.html_contributions.len(), 1);
        assert_eq!(plugin.html_contributions[0].element, "diagram");
        assert!(plugin.html_contributions[0].trusted);
        let component = plugin.html_contributions[0]
            .web_component
            .as_ref()
            .expect("mermaid declares a web component");
        assert_eq!(component.tag, "notist-mermaid");
        assert_eq!(component.module, "assets/mermaid.js");

        let mut registry = core_registry();
        let mut shaping = ShapingRegistry::new();
        register_loaded_contributions(&mut registry, &mut shaping, &[plugin]).unwrap();
        assert!(registry.get("mermaid").is_some(), "bare handler alias");
        let evaluator = Evaluator::new(registry);

        let evaluation = evaluator.evaluate_with_shaping(
            "#mermaid(source: r#\"\"\"
flowchart LR
  A --> B --> C
\"\"\"#)[caption]",
            &shaping,
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let diagram = &evaluation.forest[0];
        assert_eq!(diagram.name, "mermaid::diagram");
        assert!(diagram.block);
        assert_eq!(
            diagram.get("theme"),
            Some(&NodeValue::String("default".into()))
        );
        let Some(NodeValue::String(source)) = diagram.get("source") else {
            panic!("diagram must carry the source");
        };
        assert!(source.contains("flowchart LR"));
        assert!(!diagram.range.is_empty());
        assert!(diagram.children.iter().any(|child| child.is_core("text")
            && child.get("text") == Some(&NodeValue::String("caption".into()))));

        let themed = evaluator.evaluate_with_shaping(
            "#mermaid(source: \"pie; A: 40, B: 60\", theme: \"dark\")[]",
            &shaping,
        );
        assert!(themed.diagnostics.is_empty(), "{:?}", themed.diagnostics);
        assert_eq!(
            themed.forest[0].get("theme"),
            Some(&NodeValue::String("dark".into()))
        );

        let broken = evaluator.evaluate_with_shaping(
            "#mermaid(source: \"not a mermaid diagram @@@\")[]",
            &shaping,
        );
        assert!(
            !broken.diagnostics.is_empty(),
            "parse failures must surface as diagnostics"
        );

        let unknown_theme = evaluator
            .evaluate_with_shaping("#mermaid(source: \"pie\", theme: \"nope\")[]", &shaping);
        assert!(
            !unknown_theme.diagnostics.is_empty(),
            "unknown themes must surface as diagnostics"
        );
    }

    #[test]
    fn site_styles_parses_normalizes_and_deduplicates() {
        let styles = site_styles(Some(
            r#"
[site]
styles = ["assets/user.css", "./theme/deep/nested.css", "assets/user.css"]
"#,
        ))
        .unwrap();
        assert_eq!(styles, vec!["assets/user.css", "theme/deep/nested.css"]);
        assert!(
            site_styles(Some(
                r#"[plugins.demo]
path = "demo""#
            ))
            .unwrap()
            .is_empty()
        );
        assert!(site_styles(None).unwrap().is_empty());
    }

    #[test]
    fn site_styles_rejects_unsafe_or_invalid_entries() {
        for (config, expected) in [
            (
                r#"[site]
styles = ["../escape.css"]"#,
                "must stay inside the vault root",
            ),
            (
                r#"[site]
styles = ["/abs/user.css"]"#,
                "must be relative to the vault root",
            ),
            (
                r#"[site]
styles = ['assets\user.css']"#,
                "must use `/` separators",
            ),
            (
                r#"[site]
styles = ["assets/user.scss"]"#,
                "must name a `.css` file",
            ),
            (
                r#"[site]
styles = [""]"#,
                "must not be empty",
            ),
        ] {
            let error = site_styles(Some(config)).unwrap_err();
            assert!(error.contains(expected), "{config} -> {error}");
        }
    }
}

#[allow(dead_code, clippy::all)]
mod wit_bindings_semantic {
    wasmtime::component::bindgen!({ path: "wit/notist-plugin.wit", world: "plugin" });
}
