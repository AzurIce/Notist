use super::{Content, Env, Location, ModuleProvider, Runtime, Target, Value};
use notist_model::{ModuleAddress, ModuleAddressError as PathError, ModuleRoot};

#[derive(Clone, Debug)]
pub struct TargetObservation {
    pub location: Location,
    pub node_id: usize,
    pub result: Result<Target, PathError>,
}
#[derive(Clone, Debug)]
pub struct OutputLink {
    pub location: Location,
    pub target: Target,
}

/// Expand an explicit address without lexical bindings or executing the destination.
pub fn expand_module_address(
    world: &dyn ModuleProvider,
    current: &str,
    address: &ModuleAddress,
) -> Result<String, PathError> {
    if world.module_source(current).is_none() {
        return Err(PathError::UnknownModule(current.into()));
    }
    for part in &address.segments {
        if !(notist_syntax::valid_binding(part)
            || matches!(part.as_str(), "vault" | "self" | "super"))
        {
            return Err(PathError::InvalidSegment(part.clone()));
        }
    }
    let mut parts: Vec<String> = current.split("::").map(str::to_owned).collect();
    match &address.root {
        ModuleRoot::Vault => parts.truncate(1),
        ModuleRoot::Current => {}
        ModuleRoot::Parent(n) => {
            if n.get() >= parts.len() {
                return Err(PathError::EscapesPackageRoot);
            }
            parts.truncate(parts.len() - n.get());
        }
        ModuleRoot::Dependency(name) => {
            if !notist_syntax::valid_binding(name) {
                return Err(PathError::InvalidSegment(name.clone()));
            }
            let dep = world
                .dependency(&parts[0], name)
                .ok_or_else(|| PathError::UnknownBindingOrDependency(name.clone()))?;
            parts = vec![dep.into()];
        }
    }
    parts.extend(address.segments.iter().cloned());
    Ok(parts.join("::"))
}

impl Runtime<'_> {
    pub(super) fn target_module(
        &self,
        path: &[String],
        env: &Env,
        source: &str,
    ) -> Result<String, PathError> {
        self.path_key(path, env, source)
    }
    pub(super) fn path_key(
        &self,
        path: &[String],
        env: &Env,
        source: &str,
    ) -> Result<String, PathError> {
        let first = path.first().ok_or(PathError::EmptyPath)?;
        if !matches!(first.as_str(), "vault" | "self" | "super") {
            if let Some(value) = env.get(first) {
                let Value::Module(module) = value else {
                    return Err(PathError::NotModule(first.clone()));
                };
                let key = self
                    .world
                    .module_key(module)
                    .ok_or_else(|| PathError::UnknownModule(module.clone()))?;
                return Ok(if path.len() == 1 {
                    key.into()
                } else {
                    format!("{key}::{}", path[1..].join("::"))
                });
            }
        }
        let current = self
            .world
            .module_key(source)
            .ok_or_else(|| PathError::UnknownModule(source.into()))?;
        let mut rest = 1;
        let root = match first.as_str() {
            "vault" => ModuleRoot::Vault,
            "self" => ModuleRoot::Current,
            "super" => {
                rest = path.iter().take_while(|s| s.as_str() == "super").count();
                ModuleRoot::Parent(std::num::NonZeroUsize::new(rest).unwrap())
            }
            _ => ModuleRoot::Dependency(first.clone()),
        };
        expand_module_address(
            self.world,
            current,
            &ModuleAddress {
                root,
                segments: path[rest..].to_vec(),
            },
        )
    }
}

pub(super) fn output_links(content: &Content) -> Vec<OutputLink> {
    fn value(v: &Value, out: &mut Vec<OutputLink>) {
        match v {
            Value::Content(c) => visit(c, out),
            Value::Item(i) => {
                for v in i.args.values() {
                    value(v, out);
                }
            }
            Value::List(v) => {
                for v in v {
                    value(v, out);
                }
            }
            Value::Dict(v) => {
                for v in v.values() {
                    value(v, out);
                }
            }
            _ => {}
        }
    }
    fn visit(c: &Content, out: &mut Vec<OutputLink>) {
        match c {
            Content::Link { target, location } => out.push(OutputLink {
                target: target.clone(),
                location: location.clone(),
            }),
            Content::Sequence(parts) => {
                for c in parts {
                    visit(c, out);
                }
            }
            Content::Item(i) => {
                for v in i.args.values() {
                    value(v, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    visit(content, &mut out);
    out
}
