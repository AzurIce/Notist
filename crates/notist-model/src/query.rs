//! Shared query data; identities here are interpreted in an immutable input snapshot.
use crate::Location;
use serde::{Deserialize, Serialize};
use std::{fmt, num::NonZeroUsize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModuleKey(pub String);
impl From<&str> for ModuleKey {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for ModuleKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl fmt::Display for ModuleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleRoot {
    Vault,
    Current,
    Parent(NonZeroUsize),
    Dependency(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleAddress {
    pub root: ModuleRoot,
    pub segments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ModuleAddressError {
    EmptyPath,
    UnknownModule(String),
    UnknownBindingOrDependency(String),
    NotModule(String),
    EscapesPackageRoot,
    InvalidSegment(String),
}
impl fmt::Display for ModuleAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => f.write_str("empty module path"),
            Self::UnknownModule(v) => write!(f, "missing module `{v}`"),
            Self::UnknownBindingOrDependency(v) => write!(f, "unknown binding or dependency `{v}`"),
            Self::NotModule(v) => write!(f, "`{v}` is not a module"),
            Self::EscapesPackageRoot => f.write_str("super escapes package root"),
            Self::InvalidSegment(v) => write!(f, "invalid module segment `{v}`"),
        }
    }
}
impl std::error::Error for ModuleAddressError {}

/// UTF-8 byte offsets into one frozen source, with an exclusive end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub source: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    Syntax,
    Constructor,
    Plugin,
    Formation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationOrigin {
    pub node_id: Option<usize>,
    pub kind: OriginKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    Setup,
    Syntax,
    Evaluation,
    ContentConstraint,
    InvalidLabel,
    TargetFormation,
    MissingModule,
    MissingLabel,
    AmbiguousLabel,
    IncompleteEvaluation,
}
impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Syntax => "syntax",
            Self::Evaluation => "evaluation",
            Self::ContentConstraint => "content_constraint",
            Self::InvalidLabel => "invalid_label",
            Self::TargetFormation => "target_formation",
            Self::MissingModule => "missing_module",
            Self::MissingLabel => "missing_label",
            Self::AmbiguousLabel => "ambiguous_label",
            Self::IncompleteEvaluation => "incomplete_evaluation",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticLabel {
    pub span: SourceSpan,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub location: Option<Location>,
    pub span: Option<SourceSpan>,
    pub related: Vec<DiagnosticLabel>,
    pub notes: Vec<String>,
}
impl Diagnostic {
    pub fn error(
        code: DiagnosticCode,
        message: impl Into<String>,
        location: Option<Location>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            location,
            span: None,
            related: vec![],
            notes: vec![],
        }
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = &self.location {
            write!(f, "{}:{}: ", location.source, location.offset)?;
        }
        f.write_str(&self.message)
    }
}
