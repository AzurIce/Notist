use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
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
