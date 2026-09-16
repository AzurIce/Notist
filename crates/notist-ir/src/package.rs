//! 包层：清单、依赖、声明模块、实现描述与诊断。
//!
//! 三件事在这里定死：
//! - 静态面只看声明——check 与查询不需要加载实现；
//! - 实现按名绑定并在加载期校验，声明里不写实现是谁；
//! - 缺实现是正常状态，调用时才降级。
//!
//! 依赖走 Cargo 的形状，本切片只支持 `path` 来源。
//! spike 用一份 TOML 冒充 wasm 产物携带的接口描述——机制相同，先不碰真 wasm。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use notist_model::TextRange;
use serde::Deserialize;

use crate::ir::Diagnostic;
use crate::syntax::{self, DeclHead, DeclParam};

/// 包清单。
#[derive(Debug, Deserialize)]
pub struct PackageManifest {
    pub package: PackageSection,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    pub implementation: Option<ImplementationSection>,
}

#[derive(Debug, Deserialize)]
pub struct PackageSection {
    pub name: String,
    pub version: String,
    /// 声明模块。文档包没有这一项。
    #[serde(default)]
    pub entry: Option<String>,
}

/// 一条依赖。本切片只认 `path`。
#[derive(Clone, Debug, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// 实现绑定：实现产物放哪、符号怎么映射。声明里不写实现是谁。
#[derive(Debug, Deserialize)]
pub struct ImplementationSection {
    #[serde(default = "external_kind")]
    pub kind: String,
    pub artifact: String,
    #[serde(default)]
    pub symbols: BTreeMap<String, String>,
}

fn external_kind() -> String {
    "external".to_owned()
}

/// 实现产物携带的接口描述（冒充 wasm 的自定义 section）。
#[derive(Debug, Deserialize)]
pub struct ArtifactDescriptor {
    pub interface: String,
    #[serde(default)]
    pub symbols: Vec<String>,
}

/// 一条对外导出：声明的规范化形态就是接口。
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub head: DeclHead,
    pub name: String,
    pub params: Vec<DeclParam>,
    pub result: String,
    pub trailing: Option<String>,
    pub range: TextRange,
}

impl Export {
    fn from_declaration(declaration: &syntax::ExternDecl) -> Self {
        let trailing = declaration
            .params
            .last()
            .filter(|param| param.ty == "Content")
            .map(|param| param.name.clone());
        Self {
            head: declaration.head,
            name: declaration.name.clone(),
            params: declaration.params.clone(),
            result: declaration.result.clone(),
            trailing,
            range: declaration.range,
        }
    }

    /// 规范化的接口文本。它是声明与实现之间要比对的东西。
    pub fn canonical(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|param| format!("{}: {}", param.name, param.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = format!(
            "{} {}({params}) -> {}",
            self.head.head_word(),
            self.name,
            self.result
        );
        match self.head {
            DeclHead::Function => {}
            DeclHead::Element => out.push_str(" flow=standalone"),
            DeclHead::Inline => out.push_str(" flow=inline"),
        }
        if let Some(trailing) = &self.trailing {
            out.push_str(&format!(" trailing={trailing}"));
        }
        out
    }
}

/// 实现解析结果。
#[derive(Debug)]
pub struct ImplementationReport {
    pub kind: String,
    pub artifact: String,
    pub interface_match: bool,
    /// 声明名到解析到的符号名；`None` 表示产物里没有这个符号。
    pub symbols: Vec<(String, Option<String>)>,
}

/// 实现的存在状态，供 workspace 汇总。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImplementationStatus {
    /// 没有声明实现。
    Absent,
    /// 声明与实现接口一致、符号齐全。
    Verified,
    /// 声明与实现对不上。
    Mismatched,
}

impl ImplementationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Verified => "verified",
            Self::Mismatched => "mismatched",
        }
    }
}

/// 一个包的完整报告。
#[derive(Debug)]
pub struct PackageReport {
    pub name: String,
    pub version: String,
    pub directory: PathBuf,
    pub entry: Option<String>,
    pub dependencies: Vec<(String, Dependency)>,
    pub exports: Vec<Export>,
    pub implementation: Option<ImplementationReport>,
    pub diagnostics: Vec<Diagnostic>,
}

impl PackageReport {
    /// 规范化的接口文本：所有导出按声明顺序拼接。
    pub fn interface(&self) -> String {
        self.exports
            .iter()
            .map(Export::canonical)
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn implementation_status(&self) -> ImplementationStatus {
        match &self.implementation {
            None => ImplementationStatus::Absent,
            Some(implementation)
                if implementation.interface_match
                    && implementation.symbols.iter().all(|(_, symbol)| symbol.is_some()) =>
            {
                ImplementationStatus::Verified
            }
            Some(_) => ImplementationStatus::Mismatched,
        }
    }
}

/// 装载一个包目录。
pub fn load(directory: &Path) -> Result<PackageReport, String> {
    let manifest_path = directory.join("Notist.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: PackageManifest = toml::from_str(&manifest_text)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;

    let entry_source = match &manifest.package.entry {
        Some(entry) => {
            let entry_path = directory.join(entry);
            Some(
                std::fs::read_to_string(&entry_path)
                    .map_err(|error| format!("{}: {error}", entry_path.display()))?,
            )
        }
        None => None,
    };
    let (declarations, mut diagnostics) = match &entry_source {
        Some(source) => syntax::parse_declarations(source),
        None => (Vec::new(), Vec::new()),
    };

    let exports: Vec<Export> = declarations.iter().map(Export::from_declaration).collect();
    let interface = exports
        .iter()
        .map(Export::canonical)
        .collect::<Vec<_>>()
        .join("; ");
    let entry_range = TextRange::new(0, entry_source.as_ref().map(String::len).unwrap_or(0));

    let implementation = match &manifest.implementation {
        None => None,
        Some(section) => {
            let artifact_path = directory.join(&section.artifact);
            let artifact_text = std::fs::read_to_string(&artifact_path)
                .map_err(|error| format!("{}: {error}", artifact_path.display()))?;
            let artifact: ArtifactDescriptor = toml::from_str(&artifact_text)
                .map_err(|error| format!("{}: {error}", artifact_path.display()))?;

            let interface_match = artifact.interface.trim() == interface;
            if !interface_match {
                diagnostics.push(Diagnostic::warn(
                    "implementation-mismatch",
                    format!(
                        "artifact interface differs from the declarations: declared `{interface}`, artifact `{}`",
                        artifact.interface.trim()
                    ),
                    entry_range,
                ));
            }

            let mut symbols = Vec::new();
            for export in &exports {
                let symbol = section
                    .symbols
                    .get(&export.name)
                    .cloned()
                    .unwrap_or_else(|| export.name.clone());
                let present = artifact.symbols.iter().any(|declared| declared == &symbol);
                if !present {
                    diagnostics.push(Diagnostic::warn(
                        "implementation-missing",
                        format!(
                            "artifact `{}` does not export `{symbol}` for declared `{}`",
                            section.artifact, export.name
                        ),
                        export.range,
                    ));
                }
                symbols.push((export.name.clone(), present.then_some(symbol)));
            }

            Some(ImplementationReport {
                kind: section.kind.clone(),
                artifact: section.artifact.clone(),
                interface_match,
                symbols,
            })
        }
    };

    Ok(PackageReport {
        name: manifest.package.name,
        version: manifest.package.version,
        directory: directory.to_path_buf(),
        entry: manifest.package.entry,
        dependencies: manifest.dependencies.into_iter().collect(),
        exports,
        implementation,
        diagnostics,
    })
}

/// workspace：从根包出发的传递依赖闭包。
#[derive(Debug)]
pub struct PackageGraph {
    pub root: String,
    pub packages: Vec<PackageEntry>,
    pub edges: Vec<(String, String)>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 图里的一个包。
#[derive(Debug)]
pub struct PackageEntry {
    pub name: String,
    pub version: String,
    pub directory: PathBuf,
    pub exports: usize,
    pub implementation: ImplementationStatus,
}

/// 从根包出发装载整棵依赖树。
///
/// 只做图：路径来源、传递闭包、环检测、重名检测、名字一致性检查。
/// 没有版本求解——版本是包身份的一部分，不是求解出来的结果。
pub fn load_graph(root: &Path) -> Result<PackageGraph, String> {
    let root_report = load(root)?;
    let mut builder = GraphBuilder {
        packages: BTreeMap::new(),
        names: BTreeMap::new(),
        edges: BTreeSet::new(),
        diagnostics: Vec::new(),
    };
    let mut stack = Vec::new();
    builder.visit(root, ".", None, &mut stack);
    let root_name = root_report.name.clone();
    let mut packages: Vec<PackageEntry> = builder.packages.into_values().collect();
    packages.sort_by_key(|package| package.name.clone());
    Ok(PackageGraph {
        root: root_name,
        packages,
        edges: builder.edges.into_iter().collect(),
        diagnostics: builder.diagnostics,
    })
}

struct GraphBuilder {
    packages: BTreeMap<PathBuf, PackageEntry>,
    /// 包名到"声明里写的路径"，诊断只用它，不出现文件系统绝对路径。
    names: BTreeMap<String, String>,
    edges: BTreeSet<(String, String)>,
    diagnostics: Vec<Diagnostic>,
}

impl GraphBuilder {
    /// 从错误里去掉文件路径，只留原因。
    fn reason(error: &str) -> &str {
        error
            .split_once(": ")
            .map(|(_, rest)| rest)
            .unwrap_or(error)
    }

    fn visit(
        &mut self,
        directory: &Path,
        declared_path: &str,
        declared_name: Option<&str>,
        stack: &mut Vec<(PathBuf, String, String)>,
    ) {
        let directory = normalize(directory);
        let here = TextRange::new(0, 0);
        if let Some((_, name, _)) = stack.iter().find(|(path, _, _)| path == &directory) {
            self.diagnostics.push(Diagnostic::warn(
                "dependency-cycle",
                format!("dependency cycle through package `{name}`"),
                here,
            ));
            return;
        }
        if self.packages.contains_key(&directory) {
            return;
        }
        let report = match load(&directory) {
            Ok(report) => report,
            Err(error) => {
                let subject = match declared_name {
                    Some(name) => format!("dependency `{name}` at `{declared_path}`"),
                    None => format!("`{declared_path}`"),
                };
                self.diagnostics.push(Diagnostic::warn(
                    "dependency-not-found",
                    format!("cannot load {subject}: {}", Self::reason(&error)),
                    here,
                ));
                return;
            }
        };
        self.diagnostics.extend(report.diagnostics.iter().cloned());
        if let Some(existing) = self.names.get(&report.name)
            && existing != declared_path
        {
            self.diagnostics.push(Diagnostic::warn(
                "duplicate-package-name",
                format!(
                    "package name `{}` is provided by both `{existing}` and `{declared_path}`",
                    report.name
                ),
                here,
            ));
        }
        self.names
            .insert(report.name.clone(), declared_path.to_owned());
        self.packages.insert(
            directory.clone(),
            PackageEntry {
                name: report.name.clone(),
                version: report.version.clone(),
                directory: directory.clone(),
                exports: report.exports.len(),
                implementation: report.implementation_status(),
            },
        );

        stack.push((
            directory.clone(),
            report.name.clone(),
            declared_path.to_owned(),
        ));
        for (declared, dependency) in &report.dependencies {
            let Some(path) = &dependency.path else {
                self.diagnostics.push(Diagnostic::warn(
                    "dependency-without-path",
                    format!("dependency `{declared}` has no `path` source; only path dependencies are supported"),
                    here,
                ));
                continue;
            };
            let target = normalize(&directory.join(path));
            self.visit(&target, path, Some(declared), stack);
            match self.packages.get(&target) {
                Some(entry) => {
                    if &entry.name != declared {
                        self.diagnostics.push(Diagnostic::warn(
                            "dependency-name-mismatch",
                            format!(
                                "dependency is declared as `{declared}` but `{path}` is named `{}`",
                                entry.name
                            ),
                            here,
                        ));
                    }
                    let from = report.name.clone();
                    let to = entry.name.clone();
                    self.edges.insert((from, to));
                }
                None => {
                    let from = report.name.clone();
                    self.edges.insert((from, declared.clone()));
                }
            }
        }
        stack.pop();
    }
}

/// 路径规范化：去 `..` 与 `.`，不做符号链接解析，因为目标可能还不存在。
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
