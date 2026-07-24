use std::collections::HashMap;

use notist_model::{FunctionSignature, Type, builtin_signatures};
use notist_syntax::{Call, Expression, ExpressionKind, Markup, Parse};

use crate::DiagnosticKind;

/// Function signatures available to static checking, keyed by qualified name.
#[derive(Clone, Debug, Default)]
pub struct SignatureSet {
    signatures: HashMap<String, FunctionSignature>,
}

impl SignatureSet {
    /// Creates a signature set containing all built-in functions.
    pub fn with_builtins() -> Self {
        let mut set = Self::default();
        for (name, signature) in builtin_signatures() {
            set.signatures.insert(name.to_owned(), signature);
        }
        set
    }

    /// Adds or replaces a function signature.
    pub fn insert(&mut self, name: &str, signature: FunctionSignature) {
        self.signatures.insert(name.to_owned(), signature);
    }

    /// Looks up a function signature by its qualified name.
    pub fn get(&self, name: &str) -> Option<&FunctionSignature> {
        self.signatures.get(name)
    }

    /// Iterates over all statically visible function signatures.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &FunctionSignature)> {
        self.signatures
            .iter()
            .map(|(name, signature)| (name.as_str(), signature))
    }
}

/// A diagnostic produced by static checking without executing functions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckDiagnostic {
    /// The diagnostic category.
    pub kind: DiagnosticKind,
    /// A user-facing diagnostic message.
    pub message: String,
    /// The source range associated with the diagnostic.
    pub range: notist_model::TextRange,
}

/// Statically checks a parsed module against the available function signatures.
///
/// This pass performs name resolution, argument binding checks, and Markup
/// insertion checks without ever executing a function, so it is safe to run
/// on the LSP and `notist check` diagnostic paths.
pub fn check_module(parse: &Parse, signatures: &SignatureSet) -> Vec<CheckDiagnostic> {
    let mut checker = Checker {
        signatures,
        diagnostics: Vec::new(),
    };
    checker.check_markup(&parse.root);
    checker.diagnostics
}

struct Checker<'a> {
    signatures: &'a SignatureSet,
    diagnostics: Vec<CheckDiagnostic>,
}

impl Checker<'_> {
    fn check_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            if let notist_syntax::MarkupItem::Embedded(embedded) = item {
                let checked = self.type_of_expression(&embedded.expression);
                if let Some(ty) = checked.ty {
                    let insertable = matches!(ty, Type::Content | Type::String | Type::None);
                    if !insertable {
                        self.push(
                            DiagnosticKind::TypeMismatch,
                            format!("cannot insert {ty} into Markup"),
                            embedded.expression.range,
                        );
                    }
                }
            }
        }
    }

    /// Statically types an expression, recording diagnostics along the way.
    ///
    /// Returns `None` when the expression cannot produce a value, mirroring
    /// the evaluator: unknown calls, binding failures, and syntax errors all
    /// suppress dependent type checks so errors are not reported twice.
    fn type_of_expression(&mut self, expression: &Expression) -> CheckedType {
        match &expression.kind {
            ExpressionKind::None => CheckedType::known(Type::None),
            ExpressionKind::Bool(_) => CheckedType::known(Type::Bool),
            ExpressionKind::Int(_) => CheckedType::known(Type::Int),
            ExpressionKind::Float(_) => CheckedType::known(Type::Float),
            ExpressionKind::String(_) => CheckedType::known(Type::String),
            ExpressionKind::Content(block) => {
                self.check_markup(&block.markup);
                CheckedType::known(Type::Content)
            }
            ExpressionKind::Call(call) => self.check_call(call),
            ExpressionKind::Parenthesized(inner) => self.type_of_expression(inner),
            ExpressionKind::Error => CheckedType::unknown(),
        }
    }

    fn check_call(&mut self, call: &Call) -> CheckedType {
        let name = &call.name.value;
        let Some(signature) = self.signatures.get(name) else {
            self.push(
                DiagnosticKind::UnknownFunction,
                format!("unknown function `{name}`"),
                call.name.range,
            );
            // The call cannot produce a value, but its inputs are still
            // checked so nested errors surface exactly once.
            for argument in &call.arguments {
                self.type_of_expression(&argument.expression);
            }
            for block in &call.trailing {
                self.check_markup(&block.markup);
            }
            return CheckedType::unknown();
        };

        let mut clean = true;
        let mut provided: Vec<&'static str> = Vec::new();
        let mut positional_index = 0usize;
        let mut saw_named = false;

        for argument in &call.arguments {
            let parameter = if let Some(name) = &argument.name {
                saw_named = true;
                let found = signature
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == name.value);
                if found.is_none() {
                    self.push(
                        DiagnosticKind::InvalidArguments,
                        format!("unknown argument `{}`", name.value),
                        name.range,
                    );
                    clean = false;
                }
                found
            } else if saw_named {
                self.push(
                    DiagnosticKind::InvalidArguments,
                    "positional arguments cannot follow named arguments".into(),
                    argument.range,
                );
                clean = false;
                None
            } else {
                let parameter = signature.parameters.get(positional_index);
                positional_index += 1;
                if parameter.is_none() {
                    self.push(
                        DiagnosticKind::InvalidArguments,
                        "too many positional arguments".into(),
                        argument.range,
                    );
                    clean = false;
                }
                parameter
            };

            let Some(parameter) = parameter else { continue };
            if provided.contains(&parameter.name) {
                self.push(
                    DiagnosticKind::InvalidArguments,
                    format!("argument `{}` was provided more than once", parameter.name),
                    argument.range,
                );
                clean = false;
                continue;
            }
            let checked = self.type_of_expression(&argument.expression);
            if let Some(actual) = checked.ty {
                if !parameter.ty.accepts(&actual) {
                    self.push(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "type mismatch for argument `{}`: expected {}, found {}",
                            parameter.name, parameter.ty, actual
                        ),
                        argument.expression.range,
                    );
                    clean = false;
                }
            } else {
                clean = false;
            }
            provided.push(parameter.name);
        }

        for block in &call.trailing {
            self.check_markup(&block.markup);
            let Some(parameter_name) = signature.trailing_content else {
                self.push(
                    DiagnosticKind::InvalidArguments,
                    "function does not accept trailing Content".into(),
                    block.payload_range,
                );
                clean = false;
                continue;
            };
            let parameter = signature
                .parameters
                .iter()
                .find(|parameter| parameter.name == parameter_name);
            match parameter {
                None => {
                    self.push(
                        DiagnosticKind::InvalidArguments,
                        format!(
                            "invalid function signature: trailing Content parameter `{parameter_name}` does not exist"
                        ),
                        call.name.range,
                    );
                    clean = false;
                }
                Some(parameter) if parameter.ty != Type::Content => {
                    self.push(
                        DiagnosticKind::InvalidArguments,
                        format!(
                            "invalid function signature: trailing parameter `{parameter_name}` must have type Content"
                        ),
                        call.name.range,
                    );
                    clean = false;
                }
                Some(parameter) => {
                    if provided.contains(&parameter.name) {
                        self.push(
                            DiagnosticKind::InvalidArguments,
                            format!("argument `{}` was provided more than once", parameter.name),
                            block.payload_range,
                        );
                        clean = false;
                    } else {
                        provided.push(parameter.name);
                    }
                }
            }
        }

        for parameter in &signature.parameters {
            if provided.contains(&parameter.name) {
                continue;
            }
            if parameter.default.is_none() {
                self.push(
                    DiagnosticKind::InvalidArguments,
                    format!("missing required argument `{}`", parameter.name),
                    call.name.range,
                );
                clean = false;
            }
        }

        if clean {
            CheckedType::known(signature.result.clone())
        } else {
            CheckedType::unknown()
        }
    }

    fn push(&mut self, kind: DiagnosticKind, message: String, range: notist_model::TextRange) {
        self.diagnostics.push(CheckDiagnostic {
            kind,
            message,
            range,
        });
    }
}

/// The static type of an expression, or `None` when it cannot produce a value.
struct CheckedType {
    ty: Option<Type>,
}

impl CheckedType {
    fn known(ty: Type) -> Self {
        Self { ty: Some(ty) }
    }

    fn unknown() -> Self {
        Self { ty: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(source: &str) -> Vec<CheckDiagnostic> {
        let parse = notist_syntax::parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        check_module(&parse, &SignatureSet::with_builtins())
    }

    #[test]
    fn accepts_well_typed_documents() {
        assert!(check("#heading(level=2)[Title]").is_empty());
        assert!(check("#heading[Default level]").is_empty());
        assert!(check("#raw(text=\"code\", lang=\"rust\")").is_empty());
        assert!(check("#quote[Quoted [[vault::target]]]").is_empty());
        assert!(check("a#\"string\"#[content]#none z").is_empty());
        assert!(check("#quote(body=[ordinary])").is_empty());
    }

    #[test]
    fn reports_unknown_functions() {
        let diagnostics = check("#missing(x=1)[body]");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::UnknownFunction);
        assert_eq!(diagnostics[0].message, "unknown function `missing`");
    }

    #[test]
    fn reports_unknown_nested_functions_only_once() {
        let diagnostics = check("#heading(level=missing())[Title]");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unknown function `missing`");
    }

    #[test]
    fn rejects_non_content_values_in_markup_position() {
        let diagnostics = check("value: #42");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert_eq!(diagnostics[0].message, "cannot insert Int into Markup");
    }

    #[test]
    fn reports_argument_type_mismatches() {
        let diagnostics = check("#heading(level=\"two\")[Title]");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert_eq!(
            diagnostics[0].message,
            "type mismatch for argument `level`: expected Int, found String"
        );
    }

    #[test]
    fn reports_binding_errors() {
        let missing = check("#raw()");
        assert!(
            missing
                .iter()
                .any(|d| d.message == "missing required argument `text`")
        );

        let too_many = check("#heading(1, [body], true)");
        assert!(
            too_many
                .iter()
                .any(|d| d.message == "too many positional arguments")
        );

        let after_named = check("#heading(level=2, 3)[Title]");
        assert!(
            after_named
                .iter()
                .any(|d| d.message == "positional arguments cannot follow named arguments")
        );

        let duplicate = check("#quote(body=[a])[b]");
        assert!(
            duplicate
                .iter()
                .any(|d| d.message == "argument `body` was provided more than once")
        );

        let unknown = check("#quote(source=\"book\")[text]");
        assert!(
            unknown
                .iter()
                .any(|d| d.message == "unknown argument `source`")
        );

        let trailing = check("#raw(text=\"x\")[content]");
        assert!(
            trailing
                .iter()
                .any(|d| d.message == "function does not accept trailing Content")
        );
    }

    #[test]
    fn checks_nested_calls_and_trailing_content() {
        let nested_mismatch = check("#quote[#heading(level=\"x\")[T]]");
        assert!(
            nested_mismatch
                .iter()
                .any(|d| d.message
                    == "type mismatch for argument `level`: expected Int, found String")
        );

        let trailing_unknown = check("#quote[#missing()]");
        assert!(
            trailing_unknown
                .iter()
                .any(|d| d.message == "unknown function `missing`")
        );
    }

    #[test]
    fn signatures_can_be_extended() {
        let mut signatures = SignatureSet::with_builtins();
        signatures.insert(
            "math",
            FunctionSignature {
                parameters: vec![notist_model::Parameter {
                    name: "formula",
                    ty: Type::String,
                    default: None,
                }],
                trailing_content: None,
                result: Type::Content,
            },
        );
        let parse = notist_syntax::parse("#math(formula=\"x+1\")");
        assert!(check_module(&parse, &signatures).is_empty());
    }
}
