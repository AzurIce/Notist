use crate::DefaultValue;

/// One semantic element declared by a plugin component during initialization.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginElementDecl {
    /// Element name inside the package namespace (without the package prefix).
    pub name: String,
    pub version: u32,
    pub block: bool,
    /// Whether a runtime handler exists. Data-only declarations stay unreduced.
    pub computed: bool,
    pub parameters: Vec<PluginParamDecl>,
    pub trailing_content: Option<String>,
    pub body_mode: Option<String>,
    pub role: Option<String>,
    pub kind: Option<String>,
}

impl PluginElementDecl {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: 1,
            block: false,
            computed: true,
            parameters: Vec::new(),
            trailing_content: None,
            body_mode: None,
            role: None,
            kind: None,
        }
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn block(mut self, block: bool) -> Self {
        self.block = block;
        self
    }

    /// Marks this declaration as data-only: no runtime handler.
    pub fn data_only(mut self) -> Self {
        self.computed = false;
        self
    }

    pub fn param(mut self, name: &str, ty: &str) -> Self {
        self.parameters.push(PluginParamDecl {
            name: name.to_owned(),
            ty: ty.to_owned(),
            default: None,
        });
        self
    }

    pub fn param_default(mut self, name: &str, ty: &str, default: impl Into<DefaultValue>) -> Self {
        self.parameters.push(PluginParamDecl {
            name: name.to_owned(),
            ty: ty.to_owned(),
            default: Some(default.into()),
        });
        self
    }

    pub fn trailing_content(mut self, name: &str) -> Self {
        self.trailing_content = Some(name.to_owned());
        self
    }

    pub fn body_mode(mut self, body_mode: &str) -> Self {
        self.body_mode = Some(body_mode.to_owned());
        self
    }

    pub fn role(mut self, role: &str) -> Self {
        self.role = Some(role.to_owned());
        self
    }

    pub fn kind(mut self, kind: &str) -> Self {
        self.kind = Some(kind.to_owned());
        self
    }
}

/// One parameter declared by a plugin element.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginParamDecl {
    pub name: String,
    /// Notist type syntax: `None`, `Bool`, `Int`, `Float`, `String`,
    /// `Content`, or `T?` for optional types.
    pub ty: String,
    pub default: Option<DefaultValue>,
}
