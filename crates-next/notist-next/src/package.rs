use crate::Runtime;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub path: String,
}

pub struct Loaded {
    pub runtime: Runtime,
    pub entry: String,
    pub components: BTreeMap<String, String>,
    pub resources: BTreeMap<String, Vec<u8>>,
}
impl Loaded {
    pub fn request(&self) -> serde_json::Value {
        serde_json::json!({"request_id":"native","entry":self.entry,"files":self.runtime.sources,"dependencies":self.runtime.dependencies,
            "binaries":self.runtime.binaries.iter().map(|(p,b)|(p,crate::snapshot::base64_encode(b))).collect::<BTreeMap<_,_>>()})
    }
}

/// Derive a code identifier without changing case or transliterating Unicode.
pub fn module_name(name: &str) -> Result<String, String> {
    if !name.chars().any(|c| c.is_alphanumeric() || c == '_') {
        return Err(format!(
            "module name has no identifier characters: `{name}`"
        ));
    }
    let mut normalized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !crate::syntax::valid_binding(&normalized) {
        normalized.insert(0, '_');
    }
    Ok(normalized)
}

pub fn module_key(path: &str) -> Result<String, String> {
    let (package, local) = if let Some((package, local)) = path.split_once("/docs/") {
        (package, local)
    } else {
        ("root", path.strip_prefix("docs/").unwrap_or(path))
    };
    let stem = local
        .strip_suffix(".notc")
        .or_else(|| local.strip_suffix(".not"))
        .ok_or_else(|| format!("not a source file `{path}`"))?;
    let mut parts = stem.split('/').collect::<Vec<_>>();
    if parts.last() == Some(&"README") {
        parts.pop();
    }
    let mut key = vec![package.to_owned()];
    for part in parts {
        let name = module_name(part)
            .map_err(|error| format!("invalid module filename `{path}`: {error}"))?;
        key.push(name);
    }
    Ok(key.join("::"))
}

pub fn load(root: &Path) -> Result<Loaded, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut loaded = Loaded {
        runtime: Runtime::default(),
        entry: String::new(),
        components: BTreeMap::new(),
        resources: BTreeMap::new(),
    };
    let mut ids = BTreeMap::new();
    let mut active = BTreeSet::new();
    let id = visit(&root, &mut loaded, &mut ids, &mut active)?;
    loaded.entry = loaded
        .runtime
        .sources
        .keys()
        .find(|p| module_key(p).as_deref() == Ok(id.as_str()))
        .cloned()
        .ok_or("missing docs/README.notc or docs/README.not")?;
    Ok(loaded)
}
fn files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err(format!("symlink resource unsupported: {}", path.display()));
        }
        if !path.starts_with(root) {
            return Err("resource escapes package".into());
        }
        if path.is_dir() {
            files(root, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
fn visit(
    root: &Path,
    loaded: &mut Loaded,
    ids: &mut BTreeMap<PathBuf, String>,
    active: &mut BTreeSet<PathBuf>,
) -> Result<String, String> {
    if active.contains(root) {
        return Err(format!("cyclic package dependency: {}", root.display()));
    }
    if let Some(id) = ids.get(root) {
        return Ok(id.clone());
    }
    if ids.len() >= 128 {
        return Err("package count limit exceeded".into());
    }
    let manifest: Manifest =
        toml::from_str(&fs::read_to_string(root.join("Notist.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if manifest.package.name.is_empty() {
        return Err("empty package name".into());
    }
    let id = format!("p{}", ids.len());
    ids.insert(root.into(), id.clone());
    active.insert(root.into());
    let mut dependencies = BTreeMap::new();
    for (alias, dep) in manifest.dependencies {
        if !crate::syntax::valid_binding(&alias) {
            return Err(format!("invalid dependency alias `{alias}`"));
        }
        let path = root
            .join(dep.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        dependencies.insert(alias, visit(&path, loaded, ids, active)?);
    }
    loaded.runtime.dependencies.insert(id.clone(), dependencies);
    let mut source_files = Vec::new();
    files(root, &root.join("docs"), &mut source_files)?;
    source_files.sort();
    for path in source_files {
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("not" | "notc")
        ) {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .ok_or("non UTF-8 source path")?;
            loaded.runtime.sources.insert(
                format!("{id}/{relative}"),
                fs::read_to_string(path).map_err(|e| e.to_string())?,
            );
        }
    }
    let mut component_files = Vec::new();
    if root.join("components").exists() {
        files(root, &root.join("components"), &mut component_files)?;
    }
    for path in component_files {
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .ok_or("non UTF-8 component path")?;
        loaded.resources.insert(
            format!("{id}/{relative}"),
            fs::read(&path).map_err(|e| e.to_string())?,
        );
    }
    for resource in loaded
        .resources
        .keys()
        .filter(|resource| resource.starts_with(&format!("{id}/components/")))
        .cloned()
        .collect::<Vec<_>>()
    {
        let relative = resource
            .strip_prefix(&format!("{id}/components/"))
            .expect("component resource prefix");
        let component_name = if let Some((name, entry)) = relative.split_once('/') {
            (entry == "main.js").then(|| name.to_owned())
        } else {
            relative.strip_suffix(".js").map(str::to_owned)
        };
        let Some(name) = component_name else { continue };
        if !crate::html::valid_name(&name) {
            return Err(format!("invalid component name `{name}` in `{relative}`"));
        }
        if let Some(other) = loaded.components.insert(name.clone(), resource.clone()) {
            return Err(format!(
                "component conflict `{name}`: {other} and {resource}"
            ));
        }
    }
    let sources = loaded
        .runtime
        .sources
        .iter()
        .filter(|(p, _)| p.starts_with(&format!("{id}/")))
        .map(|(p, s)| (p.clone(), s.clone()))
        .collect::<Vec<_>>();
    for (source, text) in sources {
        for statement in crate::syntax::parse(&text) {
            if let crate::syntax::Statement::Wasm(path) = statement {
                let virtual_path = crate::runtime::relative(&source, &path)?;
                let local = virtual_path
                    .strip_prefix(&format!("{id}/"))
                    .ok_or("WASM escapes package")?;
                let disk = root.join(local).canonicalize().map_err(|e| e.to_string())?;
                if !disk.starts_with(root) {
                    return Err("WASM escapes package".into());
                }
                loaded
                    .runtime
                    .binaries
                    .insert(virtual_path, fs::read(disk).map_err(|e| e.to_string())?);
            }
        }
    }
    active.remove(root);
    Ok(id)
}
