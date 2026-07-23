use std::fmt;

mod content;
mod signature;

pub use content::{
    Block, Content, Element, ElementNode, StructuredDocument, TableAlignment, TableCellPlacement,
    TableLayoutError, table_layout,
};
pub use signature::{
    DefaultValue, FunctionSignature, Parameter, Type, abbr_signature, audio_signature,
    builtin_signatures, callout_signature, cite_signature, code_signature, details_signature,
    empty_content_signature, enum_item_signature, figure_signature, heading_signature,
    image_signature, inline_body_signature, link_signature, list_item_signature, list_signature,
    math_signature, outline_signature, quote_signature, raw_signature, ref_signature,
    table_cell_signature, table_signature, task_item_signature, terms_item_signature,
    text_signature, time_signature, video_signature,
};

/// A half-open byte range in a source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRange {
    /// The inclusive start byte offset.
    pub start: usize,
    /// The exclusive end byte offset.
    pub end: usize,
}

impl TextRange {
    /// Creates a new half-open byte range.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Returns the number of bytes covered by this range.
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Returns whether this range contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Returns a copy shifted forward by the given byte offset.
    pub const fn shifted(self, offset: usize) -> Self {
        Self::new(self.start + offset, self.end + offset)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModulePath(Vec<String>);

impl ModulePath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn from_segments(segments: impl IntoIterator<Item = String>) -> Self {
        Self(segments.into_iter().collect())
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }

    pub fn child(&self, segments: impl IntoIterator<Item = String>) -> Self {
        let mut path = self.0.clone();
        path.extend(segments);
        Self(path)
    }

    pub fn parent(&self) -> Option<Self> {
        let mut path = self.0.clone();
        path.pop()?;
        Some(Self(path))
    }

    pub fn ancestor(&self, levels: usize) -> Option<Self> {
        if levels > self.0.len() {
            return None;
        }
        Some(Self(self.0[..self.0.len() - levels].to_vec()))
    }
}

impl fmt::Display for ModulePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("vault")?;
        for segment in &self.0 {
            write!(formatter, "::{segment}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleReference {
    Absolute(Vec<String>),
    Relative(Vec<String>),
    Parent {
        levels: usize,
        remainder: Vec<String>,
    },
}

impl ModuleReference {
    pub fn resolve_from(&self, current: &ModulePath) -> Option<ModulePath> {
        match self {
            Self::Absolute(segments) => Some(ModulePath::root().child(segments.clone())),
            Self::Relative(segments) => Some(current.child(segments.clone())),
            Self::Parent { levels, remainder } => {
                Some(current.ancestor(*levels)?.child(remainder.clone()))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiReference {
    pub module: ModuleReference,
    pub label: Option<String>,
}
