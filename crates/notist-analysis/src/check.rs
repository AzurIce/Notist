use std::collections::HashMap;

use notist_model::{DefaultValue, FunctionSignature, Parameter, Type, builtin_signatures};
use notist_syntax::{
    BinaryOperator, Call, DictEntry, Expression, ExpressionKind, Markup, MarkupItem, Parse,
    UnaryOperator, UserFunctionDefinition,
};

use crate::DiagnosticKind;

/// Function signatures available to static checking, keyed by qualified name.
#[derive(Clone, Debug, Default)]
pub struct SignatureSet {
    signatures: HashMap<String, FunctionSignature>,
}

impl SignatureSet {
    /// Creates a signature set containing all built-in functions.
    ///
    /// Both the prelude short name and the canonical `core::*` qualified name
    /// are statically visible, matching the runtime registry aliases.
    pub fn with_builtins() -> Self {
        let mut set = Self::default();
        for (name, signature) in builtin_signatures() {
            set.signatures.insert(name.to_owned(), signature.clone());
            set.signatures.insert(format!("core::{name}"), signature);
        }
        set
    }

    /// Adds or replaces a function signature.
    pub fn insert(&mut self, name: &str, signature: FunctionSignature) {
        self.signatures.insert(name.to_owned(), signature);
    }

    /// Adds source-defined functions without executing their bodies.
    pub fn extend_with_user_functions(&mut self, parse: &Parse) -> Vec<CheckDiagnostic> {
        let mut diagnostics = Vec::new();
        for definition in parse.user_functions() {
            if self.signatures.contains_key(&definition.name.value) {
                diagnostics.push(CheckDiagnostic {
                    kind: DiagnosticKind::DuplicateFunction,
                    message: format!("duplicate function `{}`", definition.name.value),
                    range: definition.name.range,
                });
                continue;
            }
            self.signatures.insert(
                definition.name.value.clone(),
                signature_for_user_function(definition),
            );
        }
        diagnostics
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

/// Module-local identity assigned by static name resolution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LocalSymbolId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    Function,
    Parameter,
    /// A value binding introduced by `let` (D0007).
    Let,
    /// A parameter of an anonymous function `(params) => body`.
    LambdaParameter,
    /// A name bound by an import selector (D0004).
    ImportBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolDefinition {
    pub id: LocalSymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub ty: Type,
    pub range: notist_model::TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolReference {
    pub symbol: LocalSymbolId,
    pub range: notist_model::TextRange,
}

/// Resolved source symbols retained independently from diagnostics and evaluation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModuleSemanticIndex {
    pub definitions: Vec<SymbolDefinition>,
    pub references: Vec<SymbolReference>,
}

/// Resolves user functions and their lexical parameters to module-local identities.
pub fn resolve_module_symbols(parse: &Parse) -> ModuleSemanticIndex {
    let mut resolver = SymbolResolver::default();
    for definition in parse.user_functions() {
        if resolver.functions.contains_key(&definition.name.value) {
            continue;
        }
        let id = resolver.define(
            definition.name.value.clone(),
            SymbolKind::Function,
            Type::Function,
            definition.name.range,
        );
        resolver.functions.insert(definition.name.value.clone(), id);
    }
    // The document body is one lexical block: top-level `let` bindings stay
    // visible to every following expression in the same module.
    resolver.variables.push(HashMap::new());
    resolver.resolve_markup(&parse.root);
    resolver.variables.pop();
    resolver.index
}

#[derive(Default)]
struct SymbolResolver {
    index: ModuleSemanticIndex,
    functions: HashMap<String, LocalSymbolId>,
    variables: Vec<HashMap<String, LocalSymbolId>>,
}

impl SymbolResolver {
    fn define(
        &mut self,
        name: String,
        kind: SymbolKind,
        ty: Type,
        range: notist_model::TextRange,
    ) -> LocalSymbolId {
        let id = LocalSymbolId(self.index.definitions.len() as u32);
        self.index.definitions.push(SymbolDefinition {
            id,
            name,
            kind,
            ty,
            range,
        });
        id
    }

    fn resolve_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            match item {
                MarkupItem::Embedded(embedded) => {
                    self.resolve_expression(&embedded.expression);
                }
                MarkupItem::Heading(sugar) => self.resolve_markup(&sugar.body),
                MarkupItem::List(sugar) => {
                    for row in &sugar.rows {
                        self.resolve_markup(&row.body);
                    }
                }
                MarkupItem::Table(sugar) => {
                    for cell in &sugar.header {
                        self.resolve_markup(&cell.body);
                    }
                    for row in &sugar.rows {
                        for cell in row {
                            self.resolve_markup(&cell.body);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn resolve_expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Content(block) => self.resolve_markup(&block.markup),
            ExpressionKind::Name(name) => {
                if let Some(symbol) = self.resolve_name(&name.value) {
                    self.index.references.push(SymbolReference {
                        symbol,
                        range: name.range,
                    });
                }
            }
            ExpressionKind::Call(call) => {
                if let Some(symbol) = self.resolve_name(&call.name.value) {
                    self.index.references.push(SymbolReference {
                        symbol,
                        range: call.name.range,
                    });
                }
                for argument in &call.arguments {
                    self.resolve_expression(&argument.expression);
                }
                for block in &call.trailing {
                    self.resolve_markup(&block.markup);
                }
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.resolve_expression(left);
                self.resolve_expression(right);
            }
            ExpressionKind::Target(_) => {}
            ExpressionKind::Unary { operand, .. } => {
                self.resolve_expression(operand);
            }
            ExpressionKind::Block(statements) => {
                // One lexical scope per block so sibling statements see
                // bindings introduced by earlier `let`s.
                self.variables.push(HashMap::new());
                for statement in statements {
                    self.resolve_expression(statement);
                }
                self.variables.pop();
            }
            ExpressionKind::Let {
                name,
                annotation,
                value,
            } => {
                // Resolve the value first: a `let` cannot observe itself.
                self.resolve_expression(value);
                self.bind(
                    name.value.clone(),
                    SymbolKind::Let,
                    annotation.clone().unwrap_or(Type::Inferred),
                    name.range,
                );
            }
            ExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.resolve_expression(condition);
                self.resolve_expression(then_branch);
                if let Some(branch) = else_branch {
                    self.resolve_expression(branch);
                }
            }
            ExpressionKind::Lambda { parameters, body } => {
                let mut scope = HashMap::new();
                for parameter in parameters {
                    if let Some(default) = &parameter.default {
                        self.resolve_expression(default);
                    }
                    if scope.contains_key(&parameter.name.value) {
                        continue;
                    }
                    let id = self.define(
                        parameter.name.value.clone(),
                        SymbolKind::LambdaParameter,
                        parameter.ty.clone(),
                        parameter.name.range,
                    );
                    scope.insert(parameter.name.value.clone(), id);
                }
                self.variables.push(scope);
                self.resolve_expression(body);
                self.variables.pop();
            }
            ExpressionKind::Import { selectors, .. } => {
                for selector in selectors {
                    let (name, range) = match &selector.alias {
                        Some(alias) => (alias.value.clone(), alias.range),
                        None => (selector.name.clone(), selector.range),
                    };
                    self.bind(name, SymbolKind::ImportBinding, Type::Inferred, range);
                }
            }
            ExpressionKind::LetFunction(definition) => {
                for parameter in &definition.parameters {
                    if let Some(default) = &parameter.default {
                        self.resolve_expression(default);
                    }
                }
                let mut scope = HashMap::new();
                // The function's own name is visible inside its body so
                // recursive calls resolve to the same identity. Top-level
                // functions were already registered by `resolve_module_symbols`;
                // reuse that identity instead of defining a duplicate.
                let function_id = if let Some(existing) = self.functions.get(&definition.name.value)
                {
                    *existing
                } else {
                    self.define(
                        definition.name.value.clone(),
                        SymbolKind::Function,
                        Type::Function,
                        definition.name.range,
                    )
                };
                scope.insert(definition.name.value.clone(), function_id);
                for parameter in &definition.parameters {
                    if scope.contains_key(&parameter.name.value) {
                        continue;
                    }
                    let id = self.define(
                        parameter.name.value.clone(),
                        SymbolKind::Parameter,
                        parameter.ty.clone(),
                        parameter.name.range,
                    );
                    scope.insert(parameter.name.value.clone(), id);
                }
                self.variables.push(scope);
                self.resolve_expression(&definition.body);
                self.variables.pop();
            }
            ExpressionKind::Parenthesized(inner) => self.resolve_expression(inner),
            ExpressionKind::Array(elements) => {
                for element in elements {
                    self.resolve_expression(element);
                }
            }
            ExpressionKind::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Spread(expr) => self.resolve_expression(expr),
                        DictEntry::Entry { key, value } => {
                            // A bare identifier key is sugar for the String
                            // key, not a name reference.
                            if !matches!(key.kind, ExpressionKind::Name(_)) {
                                self.resolve_expression(key);
                            }
                            self.resolve_expression(value);
                        }
                    }
                }
            }
            ExpressionKind::Spread(inner) => self.resolve_expression(inner),
            ExpressionKind::Unit
            | ExpressionKind::Bool(_)
            | ExpressionKind::Int(_)
            | ExpressionKind::Float(_)
            | ExpressionKind::String(_)
            | ExpressionKind::Error => {}
        }
    }

    /// Defines one symbol and binds it into the innermost open scope when
    /// one exists; definitions without an enclosing scope stay recorded but
    /// unreachable by later name lookups.
    fn bind(
        &mut self,
        name: String,
        kind: SymbolKind,
        ty: Type,
        range: notist_model::TextRange,
    ) -> LocalSymbolId {
        let id = self.define(name.clone(), kind, ty, range);
        if let Some(scope) = self.variables.last_mut()
            && !scope.contains_key(&name)
        {
            scope.insert(name, id);
        }
        id
    }

    fn resolve_name(&self, name: &str) -> Option<LocalSymbolId> {
        self.variables
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.functions.get(name).copied())
    }
}

/// Statically checks a parsed module against the available function signatures.
///
/// This pass performs name resolution, argument binding checks, and Markup
/// insertion checks without ever executing a function, so it is safe to run
/// on the LSP and `notist check` diagnostic paths.
pub fn check_module(parse: &Parse, signatures: &SignatureSet) -> Vec<CheckDiagnostic> {
    check_module_with_prelude(parse, signatures, HashMap::new())
}

/// Statically checks a module whose document scope is pre-seeded with
/// imported names (D0004): imported values carry an unchecked type until the
/// target module's real type information is available.
pub fn check_module_with_prelude(
    parse: &Parse,
    signatures: &SignatureSet,
    prelude: HashMap<String, Type>,
) -> Vec<CheckDiagnostic> {
    let mut checker = Checker {
        signatures,
        functions: HashMap::new(),
        diagnostics: Vec::new(),
        variables: vec![prelude],
    };
    let mut diagnostics = Vec::new();
    checker.check_markup(&parse.root);
    diagnostics.extend(checker.diagnostics);
    diagnostics
}

pub fn signature_for_user_function(definition: &UserFunctionDefinition) -> FunctionSignature {
    let parameters = definition
        .parameters
        .iter()
        .map(|parameter| Parameter {
            name: parameter.name.value.clone(),
            ty: parameter.ty.clone(),
            default: parameter.default.as_ref().and_then(default_value),
        })
        .collect::<Vec<_>>();
    let trailing_content = parameters
        .last()
        .filter(|parameter| parameter.ty == Type::Content)
        .map(|parameter| parameter.name.clone());
    FunctionSignature {
        parameters,
        trailing_content,
        result: definition.result.clone(),
    }
}

fn default_value(expression: &Expression) -> Option<DefaultValue> {
    match &expression.kind {
        ExpressionKind::Unit => Some(DefaultValue::None),
        ExpressionKind::Bool(value) => Some(DefaultValue::Bool(*value)),
        ExpressionKind::Int(value) => Some(DefaultValue::Int(*value)),
        ExpressionKind::Float(value) => Some(DefaultValue::Float(*value)),
        ExpressionKind::String(value) => Some(DefaultValue::String(value.value.clone())),
        ExpressionKind::Parenthesized(inner) => default_value(inner),
        _ => None,
    }
}

struct Checker<'a> {
    signatures: &'a SignatureSet,
    /// Source-defined function signatures registered in declaration order
    /// (D0002 sequential scope: no hoisting).
    functions: HashMap<String, FunctionSignature>,
    diagnostics: Vec<CheckDiagnostic>,
    variables: Vec<HashMap<String, Type>>,
}

impl Checker<'_> {
    fn check_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            if let notist_syntax::MarkupItem::Heading(sugar) = item {
                self.check_markup(&sugar.body);
                continue;
            }
            if let notist_syntax::MarkupItem::List(sugar) = item {
                for row in &sugar.rows {
                    self.check_markup(&row.body);
                }
                continue;
            }
            if let notist_syntax::MarkupItem::Table(sugar) = item {
                for cell in &sugar.header {
                    self.check_markup(&cell.body);
                }
                for row in &sugar.rows {
                    for cell in row {
                        self.check_markup(&cell.body);
                    }
                }
                continue;
            }
            if let notist_syntax::MarkupItem::Annotation(annotation) = item {
                self.check_annotation_payload(annotation);
                continue;
            }
            if let notist_syntax::MarkupItem::Embedded(embedded) = item {
                let checked = self.type_of_expression(&embedded.expression);
                if let Some(ty) = checked.ty {
                    let insertable = Self::type_insertable(&ty);
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

    /// Checks an `@`/`@!` annotation payload: it must be a Dict whose keys
    /// satisfy the C5 constraint and whose values stay inside the
    /// annotation value domain (Unit / Bool / Int / Float / String and
    /// Arrays / Dicts of those, recursively). `Unit` values are legal and
    /// read as "attribute absent", which is how a deep override deletes an
    /// inherited attribute.
    fn check_annotation_payload(&mut self, annotation: &notist_syntax::Annotation) {
        let Some(ty) = self.type_of_expression(&annotation.expression).ty else {
            return;
        };
        let Type::Dict(key, value) = &ty else {
            if !matches!(ty, Type::Inferred) {
                self.push(
                    DiagnosticKind::TypeMismatch,
                    format!("annotation must evaluate to a Dict, found {ty}"),
                    annotation.expression.range,
                );
            }
            return;
        };
        if let Some(key) = key
            && !Self::is_dict_key_type(key)
        {
            self.push(
                DiagnosticKind::TypeMismatch,
                format!("cannot use {key} as an annotation key"),
                annotation.expression.range,
            );
        }
        if let Some(value) = value
            && !Self::is_annotation_value(value)
        {
            self.push(
                DiagnosticKind::TypeMismatch,
                format!(
                    "annotation value type {value} is outside the annotation domain \
                     (Unit / Bool / Int / Float / String / Array / Dict of those)"
                ),
                annotation.expression.range,
            );
        }
    }

    /// The C5 Dict key constraint: Unit / Bool / Int / String or a union of
    /// those.
    fn is_dict_key_type(ty: &Type) -> bool {
        Type::union([Type::Unit, Type::Bool, Type::Int, Type::String]).contains(ty)
    }

    /// The annotation payload value domain, recursively.
    fn is_annotation_value(ty: &Type) -> bool {
        match ty {
            Type::Unit
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Never
            | Type::Inferred => true,
            Type::Optional(inner) => Self::is_annotation_value(inner),
            Type::Union(members) => members.iter().all(|member| Self::is_annotation_value(member)),
            Type::Array(element) => element
                .as_deref()
                .map(Self::is_annotation_value)
                .unwrap_or(true),
            Type::Dict(key, value) => {
                let keys_ok = key
                    .as_deref()
                    .map(Self::is_dict_key_type)
                    .unwrap_or(true);
                let values_ok = value
                    .as_deref()
                    .map(Self::is_annotation_value)
                    .unwrap_or(true);
                keys_ok && values_ok
            }
            _ => false,
        }
    }

    fn type_insertable(ty: &Type) -> bool {
        match ty {
            Type::Content
            | Type::String
            | Type::Unit
            | Type::Int
            | Type::Float
            | Type::Bool
            | Type::Target
            | Type::Never
            | Type::Inferred => true,
            Type::Optional(inner) => Self::type_insertable(inner),
            Type::Union(members) => members.iter().all(Self::type_insertable),
            Type::Function | Type::Array(_) | Type::Dict(..) => false,
        }
    }

    /// Statically types an expression, recording diagnostics along the way.
    ///
    /// Returns `None` when the expression cannot produce a value, mirroring
    /// the evaluator: unknown calls, binding failures, and syntax errors all
    /// suppress dependent type checks so errors are not reported twice.
    fn type_of_expression(&mut self, expression: &Expression) -> CheckedType {
        match &expression.kind {
            ExpressionKind::Unit => CheckedType::known(Type::Unit),
            ExpressionKind::Spread(_) => CheckedType::unknown(),
            ExpressionKind::Array(elements) => {
                let mut members = Vec::new();
                let mut clean = true;
                for element in elements {
                    match self.type_of_expression(element).ty {
                        Some(ty) => members.push(ty),
                        None => clean = false,
                    }
                }
                if !clean {
                    return CheckedType::unknown();
                }
                // An empty literal has member type `Never` (C4): it flows
                // into any `Array<T>` expectation.
                let element = match members.len() {
                    0 => Type::Never,
                    _ => Type::union(members),
                };
                CheckedType::known(Type::Array(Some(Box::new(element))))
            }
            ExpressionKind::Dict(entries) => {
                let mut keys = Vec::new();
                let mut values = Vec::new();
                let mut clean = true;
                for entry in entries {
                    match entry {
                        DictEntry::Spread(expr) => match self.type_of_expression(expr).ty {
                            // A spread Dict's entries widen K/V in unknown
                            // ways; keep the literal typing permissive.
                            Some(Type::Dict(..)) => {
                                keys.push(Type::Inferred);
                                values.push(Type::Inferred);
                            }
                            Some(other) => {
                                self.push(
                                    DiagnosticKind::TypeMismatch,
                                    format!("`..` spread requires a Dict, found {other}"),
                                    expr.range,
                                );
                                clean = false;
                            }
                            None => clean = false,
                        },
                        DictEntry::Entry { key, value } => {
                            // A bare identifier key is sugar for the String
                            // key; it is not a name reference (mirrors the
                            // evaluator's `dict_key`).
                            match &key.kind {
                                ExpressionKind::Name(_) => keys.push(Type::String),
                                _ => match self.type_of_expression(key).ty {
                                    Some(key_type) => {
                                        if !Self::is_dict_key_type(&key_type) {
                                            self.push(
                                                DiagnosticKind::TypeMismatch,
                                                format!(
                                                    "cannot use {key_type} as a Dict key"
                                                ),
                                                key.range,
                                            );
                                            clean = false;
                                        } else {
                                            keys.push(key_type);
                                        }
                                    }
                                    None => clean = false,
                                },
                            }
                            match self.type_of_expression(value).ty {
                                Some(value_type) => values.push(value_type),
                                None => clean = false,
                            }
                        }
                    }
                }
                if !clean {
                    return CheckedType::unknown();
                }
                // Empty K/V member types are `Never` (C4).
                let key = match keys.len() {
                    0 => Type::Never,
                    _ => Type::union(keys),
                };
                let value = match values.len() {
                    0 => Type::Never,
                    _ => Type::union(values),
                };
                CheckedType::known(Type::Dict(
                    Some(Box::new(key)),
                    Some(Box::new(value)),
                ))
            }
            ExpressionKind::Bool(_) => CheckedType::known(Type::Bool),
            ExpressionKind::Int(_) => CheckedType::known(Type::Int),
            ExpressionKind::Float(_) => CheckedType::known(Type::Float),
            ExpressionKind::String(_) => CheckedType::known(Type::String),
            ExpressionKind::Target(_) => CheckedType::known(Type::Target),
            ExpressionKind::Content(block) => {
                self.check_markup(&block.markup);
                CheckedType::known(Type::Content)
            }
            ExpressionKind::Name(name) => self.resolve_name(name),
            ExpressionKind::Call(call) => self.check_call(call),
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.check_binary(*operator, left, right, expression.range),
            ExpressionKind::LetFunction(definition) => {
                self.check_user_function(definition);
                let signature = signature_for_user_function(definition);
                if self
                    .functions
                    .insert(definition.name.value.clone(), signature)
                    .is_some()
                {
                    self.push(
                        DiagnosticKind::DuplicateFunction,
                        format!("duplicate function `{}`", definition.name.value),
                        definition.name.range,
                    );
                }
                CheckedType::known(Type::Unit)
            }
            ExpressionKind::Parenthesized(inner) => self.type_of_expression(inner),
            ExpressionKind::Unary { operator, operand } => {
                let operand_type = self.type_of_expression(operand);
                if let Some(ty) = &operand_type.ty
                    && (!matches!(operator, UnaryOperator::Not) || *ty != Type::Bool)
                {
                    self.diagnostics.push(CheckDiagnostic {
                        kind: DiagnosticKind::TypeMismatch,
                        message: format!("`{operator:?}` requires a Bool operand, found {ty}"),
                        range: expression.range,
                    });
                }
                CheckedType::known(Type::Bool)
            }
            ExpressionKind::Block(statements) => {
                // D0006 join semantics: Content combines; `let` yields None;
                // a single non-Content value is the block value.
                self.variables.push(HashMap::new());
                let mut result = CheckedType::known(Type::Unit);
                for statement in statements {
                    let statement_type = self.type_of_expression(statement);
                    result = match (result.ty.clone(), statement_type.ty.clone()) {
                        (None, ty) | (ty, None) => CheckedType { ty },
                        (Some(Type::Unit), Some(ty)) => CheckedType { ty: Some(ty) },
                        (Some(ty), Some(Type::Unit)) => CheckedType { ty: Some(ty) },
                        (Some(Type::Content), Some(Type::Content)) => {
                            CheckedType::known(Type::Content)
                        }
                        (Some(left), Some(right)) => {
                            self.push(
                                DiagnosticKind::TypeMismatch,
                                format!("cannot combine {left} with {right} in a code block"),
                                statement.range,
                            );
                            CheckedType::unknown()
                        }
                    };
                }
                self.variables.pop();
                result
            }
            ExpressionKind::Let {
                name,
                annotation,
                value,
            } => {
                let value_type = self.type_of_expression(value);
                if let (Some(declared), Some(actual)) = (&annotation.clone(), &value_type.ty)
                    && !declared.accepts(actual)
                {
                    self.push(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "type mismatch in `let {}`: expected {declared}, found {actual}",
                            name.value
                        ),
                        name.range,
                    );
                }
                let ty = annotation.clone().or_else(|| value_type.ty.clone());
                if self.variables.is_empty() {
                    self.variables.push(HashMap::new());
                }
                if let (Some(ty), Some(scope)) = (ty, self.variables.last_mut()) {
                    scope.insert(name.value.clone(), ty);
                }
                CheckedType::known(Type::Unit)
            }
            ExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_type = self.type_of_expression(condition);
                if let Some(ty) = &condition_type.ty
                    && *ty != Type::Bool
                {
                    self.push(
                        DiagnosticKind::TypeMismatch,
                        format!("`if` condition must be a Bool, found {ty}"),
                        condition.range,
                    );
                }
                let then_type = self.type_of_expression(then_branch);
                match else_branch {
                    Some(branch) => {
                        let else_type = self.type_of_expression(branch);
                        match (then_type.ty.clone(), else_type.ty) {
                            (Some(left), Some(right)) if left == right => CheckedType::known(left),
                            (Some(Type::Content), _) | (_, Some(Type::Content)) => {
                                CheckedType::known(Type::Content)
                            }
                            (Some(_), Some(_)) => CheckedType::unknown(),
                            (ty, None) | (None, ty) => CheckedType { ty },
                        }
                    }
                    None => match then_type.ty {
                        // D0007: without `else` a Code branch type promotes to
                        // T?; a Content branch stays Content (empty is legal).
                        Some(Type::Content) => CheckedType::known(Type::Content),
                        Some(ty) => CheckedType::known(Type::Optional(Box::new(ty))),
                        None => CheckedType::unknown(),
                    },
                }
            }
            ExpressionKind::Lambda { parameters, body } => {
                self.variables.push(HashMap::new());
                for parameter in parameters {
                    if let Some(scope) = self.variables.last_mut() {
                        scope.insert(parameter.name.value.clone(), parameter.ty.clone());
                    }
                }
                let _ = self.type_of_expression(body);
                self.variables.pop();
                CheckedType::known(Type::Function)
            }
            ExpressionKind::Import { .. } => CheckedType::known(Type::Unit),
            ExpressionKind::Error => CheckedType::unknown(),
        }
    }

    fn resolve_name(&mut self, name: &notist_syntax::SpannedName) -> CheckedType {
        if let Some(ty) = self
            .variables
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name.value))
        {
            return CheckedType::known(ty.clone());
        }
        if self.functions.contains_key(&name.value) {
            return CheckedType::known(Type::Function);
        }
        if self.signatures.get(&name.value).is_some() {
            return CheckedType::known(Type::Function);
        }
        self.push(
            DiagnosticKind::UnresolvedName,
            format!("unresolved name `{}`", name.value),
            name.range,
        );
        CheckedType::unknown()
    }

    fn check_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
        range: notist_model::TextRange,
    ) -> CheckedType {
        let left = self.type_of_expression(left).ty;
        let right = self.type_of_expression(right).ty;
        let (Some(left), Some(right)) = (left, right) else {
            return CheckedType::unknown();
        };
        let result = match (operator, &left, &right) {
            (BinaryOperator::Add, Type::String, Type::String) => Some(Type::String),
            (BinaryOperator::Equal | BinaryOperator::NotEqual, left, right)
                if !matches!(left, Type::Content | Type::Function)
                    && !matches!(right, Type::Content | Type::Function) =>
            {
                Some(Type::Bool)
            }
            (
                BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual,
                Type::Int | Type::Float,
                Type::Int | Type::Float,
            ) => Some(Type::Bool),
            (BinaryOperator::And | BinaryOperator::Or, Type::Bool, Type::Bool) => Some(Type::Bool),
            (_, Type::Int, Type::Int) => Some(Type::Int),
            (_, Type::Int | Type::Float, Type::Int | Type::Float) => Some(Type::Float),
            _ => None,
        };
        if let Some(result) = result {
            CheckedType::known(result)
        } else {
            self.push(
                DiagnosticKind::TypeMismatch,
                format!("operator {operator:?} does not accept {left} and {right}"),
                range,
            );
            CheckedType::unknown()
        }
    }

    fn check_user_function(&mut self, definition: &UserFunctionDefinition) {
        let mut scope = HashMap::new();
        for parameter in &definition.parameters {
            if scope
                .insert(parameter.name.value.clone(), parameter.ty.clone())
                .is_some()
            {
                self.push(
                    DiagnosticKind::InvalidFunction,
                    format!("duplicate parameter `{}`", parameter.name.value),
                    parameter.name.range,
                );
            }
            if let Some(default) = &parameter.default
                && let Some(actual) = self.type_of_expression(default).ty
                && !parameter.ty.accepts(&actual)
            {
                self.push(
                    DiagnosticKind::TypeMismatch,
                    format!(
                        "default value for `{}` is {actual}, expected {}",
                        parameter.name.value, parameter.ty
                    ),
                    default.range,
                );
            }
        }
        self.variables.push(scope);
        let body = self.type_of_expression(&definition.body).ty;
        self.variables.pop();
        if let Some(actual) = body
            && !definition.result.accepts(&actual)
        {
            self.push(
                DiagnosticKind::TypeMismatch,
                format!(
                    "function `{}` returns {actual}, expected {}",
                    definition.name.value, definition.result
                ),
                definition.body.range,
            );
        }
    }

    fn check_call(&mut self, call: &Call) -> CheckedType {
        let name = &call.name.value;
        // Calls to a `let`-bound function value (D0002 closures) are known
        // callables: their runtime closure carries the signature, so static
        // checking only validates the argument expressions.
        if self
            .variables
            .iter()
            .rev()
            .any(|scope| matches!(scope.get(name), Some(Type::Function | Type::Inferred)))
        {
            for argument in &call.arguments {
                self.type_of_expression(&argument.expression);
            }
            for block in &call.trailing {
                self.check_markup(&block.markup);
            }
            return CheckedType::known(Type::Content);
        }
        let Some(signature) = self
            .functions
            .get(name)
            .cloned()
            .or_else(|| self.signatures.get(name).cloned())
        else {
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
        let mut provided: Vec<&str> = Vec::new();
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
            } else if saw_named && matches!(argument.expression.kind, ExpressionKind::Content(_)) {
                // R05: the trailing Content block is the one positional
                // argument allowed after named arguments.
                let trailing = signature.trailing_content.as_deref().and_then(|name| {
                    signature
                        .parameters
                        .iter()
                        .find(|parameter| parameter.name == name)
                });
                if trailing.is_none() {
                    self.push(
                        DiagnosticKind::InvalidArguments,
                        "positional arguments cannot follow named arguments".into(),
                        argument.range,
                    );
                    clean = false;
                }
                trailing
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
            if provided.contains(&parameter.name.as_str()) {
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
            provided.push(parameter.name.as_str());
        }

        for block in &call.trailing {
            self.check_markup(&block.markup);
            let Some(parameter_name) = signature.trailing_content.as_deref() else {
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
                    if provided.contains(&parameter.name.as_str()) {
                        self.push(
                            DiagnosticKind::InvalidArguments,
                            format!("argument `{}` was provided more than once", parameter.name),
                            block.payload_range,
                        );
                        clean = false;
                    } else {
                        provided.push(parameter.name.as_str());
                    }
                }
            }
        }

        for parameter in &signature.parameters {
            if provided.contains(&parameter.name.as_str()) {
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
    fn array_literals_join_element_types_into_unions() {
        // `(1, "a")` is Array<Int | String>: the joined element type shows
        // up in the mismatch diagnostic against `raw`'s String parameter.
        let diagnostics = check("#raw((1, \"a\"))");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("found Array<Int | String>")),
            "{diagnostics:?}"
        );
        let diagnostics = check("#raw((1, 2))");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("found Array<Int>")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn typed_collections_check_let_annotations() {
        assert!(check("#let xs: Array<Int> = (1, 2)").is_empty());
        assert!(check("#let xs: Array<Int> = (,)").is_empty());
        assert!(check("#let d: Dict<String, Int | Bool> = (\"n\": 1)").is_empty());
        // A narrower element type cannot fill a wider-parameterized
        // expectation's coercion hole (C2: Array<Int> is not Array<Float>).
        let diagnostics = check("#let xs: Array<Float> = (1, 2)");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("expected Array<Float>, found Array<Int>")),
            "{diagnostics:?}"
        );
        let diagnostics = check("#let n: Int = \"a\"");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("type mismatch in `let n`")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn dict_literals_enforce_the_key_constraint() {
        assert!(check("#let d = (\"theme\": \"dark\")").is_empty());
        let diagnostics = check("#let d = (1.5: \"x\")");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("cannot use Float as a Dict key")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn annotation_payloads_stay_inside_the_value_domain() {
        // Scalars and nested collections of scalars are legal; a Unit value
        // is legal too and reads as "attribute absent".
        assert!(check("@(id: \"install\", wip: true)").is_empty());
        assert!(check("@(meta: (\"depth\": 2), tags: (\"a\", \"b\"))").is_empty());
        assert!(check("@(empty: ())").is_empty());
        // Item payloads are outside the domain.
        let diagnostics = check("@(body: [text])");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("outside the annotation domain")),
            "{diagnostics:?}"
        );
    }


    #[test]
    fn accepts_well_typed_documents() {
        assert!(check("#heading(level=2)[Title]").is_empty());
        assert!(check("#heading[Default level]").is_empty());
        assert!(check("#raw(source=\"code\", lang=\"rust\")").is_empty());
        assert!(check("#details[Quoted [[vault::target]]]").is_empty());
        assert!(check("a#\"string\"#[content]#() z").is_empty());
        assert!(check("#details(body=[ordinary])").is_empty());
        // D0007: `fn(parameters) -> R` is the written function type (R07).
        assert!(
            check("#let f: fn(x: Int =, trailing body: Content) -> Content = (x: Int) => x * 2")
                .is_empty()
        );
    }

    #[test]
    fn checks_expressions_inside_pipe_table_cells() {
        let diagnostics =
            check("| Name | #missing() |\n| --- | --- |\n| #heading(level=\"x\")[T] | body |\n");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::UnknownFunction
                && diagnostic.message == "unknown function `missing`"
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.message == "type mismatch for argument `level`: expected Int, found String"
        }));
    }

    #[test]
    fn accepts_well_typed_explicit_table_constructors() {
        assert!(
            check("#table(columns: 2, align: \"left, right\")[#table-cell[A] #table-cell[B]]")
                .is_empty()
        );
        // Typst alignment: caption belongs to figure, not table.
        let caption_on_table = check("#table(columns: 1, caption: [C])[#table-cell[A]]");
        assert!(
            caption_on_table
                .iter()
                .any(|diagnostic| diagnostic.message == "unknown argument `caption`")
        );
    }

    #[test]
    fn accepts_typst_style_figure_wrappers() {
        assert!(check(
            "#figure(caption: [Cap], supplement: [Tab], kind: \"table\")[#table(columns: 1)[#table-cell[A]]]"
        )
        .is_empty());
        let bad_kind = check("#figure(kind: 2)[body]");
        assert!(
            bad_kind
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expected String"))
        );
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
    fn stringifies_scalars_and_rejects_functions_in_markup_position() {
        // D0002 insertion rules: Int/Float/Bool stringify into Text.
        assert!(check("value: #42").is_empty());
        assert!(check("value: #(1.5)").is_empty());
        assert!(check("value: #true").is_empty());
        // Functions cannot be inserted.
        let diagnostics = check("value: #heading");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert_eq!(diagnostics[0].message, "cannot insert Function into Markup");
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
                .any(|d| d.message == "missing required argument `source`")
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

        let duplicate = check("#details(body=[a])[b]");
        assert!(
            duplicate
                .iter()
                .any(|d| d.message == "argument `body` was provided more than once")
        );

        let multiple_trailing = check("#details[a][b]");
        assert!(multiple_trailing.iter().any(|diagnostic| {
            diagnostic.message == "argument `body` was provided more than once"
        }));

        let unknown = check("#details(source=\"book\")[text]");
        assert!(
            unknown
                .iter()
                .any(|d| d.message == "unknown argument `source`")
        );

        let trailing = check("#raw(source=\"x\")[content]");
        assert!(
            trailing
                .iter()
                .any(|d| d.message == "function does not accept trailing Content")
        );
    }

    #[test]
    fn checks_nested_calls_and_trailing_content() {
        let nested_mismatch = check("#details[#heading(level=\"x\")[T]]");
        assert!(
            nested_mismatch
                .iter()
                .any(|d| d.message
                    == "type mismatch for argument `level`: expected Int, found String")
        );

        let trailing_unknown = check("#details[#missing()]");
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
            "formula",
            FunctionSignature {
                parameters: vec![notist_model::Parameter {
                    name: "formula".into(),
                    ty: Type::String,
                    default: None,
                }],
                trailing_content: None,
                result: Type::Content,
            },
        );
        let parse = notist_syntax::parse("#formula(formula=\"x+1\")");
        assert!(check_module(&parse, &signatures).is_empty());
    }

    #[test]
    fn checks_user_function_scopes_defaults_calls_and_results() {
        let valid = check(
            "#let add(a: Int, b: Float = 1.5) -> Float = a + b\n\
             #let twice(value: Float) -> Float = add(2, value)\n\
             #let ignore(value: Float) -> Content = []\n\
             #ignore(twice(2.0))",
        );
        assert!(valid.is_empty(), "{valid:?}");

        let unresolved = check("#let broken(value: Int) -> Int = value + missing");
        assert!(
            unresolved
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedName)
        );

        let wrong_result = check("#let broken() -> Int = \"wrong\"");
        assert!(wrong_result.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::TypeMismatch
                && diagnostic.message == "function `broken` returns String, expected Int"
        }));

        let wrong_default = check("#let broken(value: Int = \"wrong\") -> Int = value");
        assert!(wrong_default.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::TypeMismatch
                && diagnostic
                    .message
                    .contains("default value for `value` is String, expected Int")
        }));
    }

    #[test]
    fn reports_duplicate_user_functions_and_parameters() {
        let diagnostics = check(
            "#let same(value: Int, value: Int) -> Int = value\n\
             #let same() -> Int = 0",
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == DiagnosticKind::DuplicateFunction)
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == DiagnosticKind::InvalidFunction
                && diagnostic.message == "duplicate parameter `value`"
        }));
    }

    #[test]
    fn resolves_function_and_parameter_uses_to_symbol_identity() {
        let source = "#let add(a: Int, b: Int) -> Int = a + b\n#add(1, 2)";
        let parse = notist_syntax::parse(source);
        let index = resolve_module_symbols(&parse);
        let function = index
            .definitions
            .iter()
            .find(|definition| definition.name == "add")
            .unwrap();
        let a = index
            .definitions
            .iter()
            .find(|definition| definition.name == "a")
            .unwrap();
        let b = index
            .definitions
            .iter()
            .find(|definition| definition.name == "b")
            .unwrap();

        assert_eq!(function.kind, SymbolKind::Function);
        assert_eq!(a.kind, SymbolKind::Parameter);
        assert_eq!(b.kind, SymbolKind::Parameter);
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == function.id)
                .count(),
            1
        );
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == a.id)
                .count(),
            1
        );
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == b.id)
                .count(),
            1
        );
    }

    #[test]
    fn resolves_let_lambda_and_import_bindings() {
        let source = concat!(
            "#let greeting = \"hi\"\n",
            "#greeting #greeting\n",
            "#import <vault::extras>::{echo as shout, plain}\n",
            "#let twice = (n: Int) => {\n",
            "  let doubled = n * 2\n",
            "  doubled\n",
            "}\n",
            "#twice(2) #shout(1)\n",
            "#let rec(n: Int) -> Int = rec(n)\n"
        );
        let parse = notist_syntax::parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let index = resolve_module_symbols(&parse);

        let kind_of = |name: &str, index: &ModuleSemanticIndex| {
            index
                .definitions
                .iter()
                .find(|definition| definition.name == name)
                .unwrap_or_else(|| panic!("missing definition `{name}`"))
                .kind
        };
        assert_eq!(kind_of("greeting", &index), SymbolKind::Let);
        assert_eq!(kind_of("twice", &index), SymbolKind::Let);
        assert_eq!(kind_of("doubled", &index), SymbolKind::Let);
        assert_eq!(kind_of("n", &index), SymbolKind::LambdaParameter);
        assert_eq!(kind_of("shout", &index), SymbolKind::ImportBinding);
        assert_eq!(kind_of("plain", &index), SymbolKind::ImportBinding);

        // `#greeting` references resolve to the `let` binding.
        let greeting = index
            .definitions
            .iter()
            .find(|definition| definition.name == "greeting")
            .unwrap();
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == greeting.id)
                .count(),
            2
        );

        // A call to a let-bound lambda resolves to that binding.
        let twice = index
            .definitions
            .iter()
            .find(|definition| definition.name == "twice")
            .unwrap();
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == twice.id)
                .count(),
            1
        );

        // An import alias used as a call resolves to the import binding.
        let shout = index
            .definitions
            .iter()
            .find(|definition| definition.name == "shout")
            .unwrap();
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == shout.id)
                .count(),
            1
        );

        // The lambda body's inner `let` is registered with its own identity.
        let doubled = index
            .definitions
            .iter()
            .find(|definition| definition.name == "doubled")
            .unwrap();
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == doubled.id)
                .count(),
            1
        );

        // Recursive let-functions resolve their self-call.
        let rec = index
            .definitions
            .iter()
            .find(|definition| definition.name == "rec" && definition.kind == SymbolKind::Function)
            .unwrap();
        assert_eq!(
            index
                .references
                .iter()
                .filter(|reference| reference.symbol == rec.id)
                .count(),
            1
        );
    }

    #[test]
    fn lets_shadow_outer_bindings_inside_blocks() {
        let source = concat!(
            "#let x = 1\n",
            "#heading(level={\n",
            "  let x = 2\n",
            "  x\n",
            "})\n",
            "#x\n"
        );
        let parse = notist_syntax::parse(source);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let index = resolve_module_symbols(&parse);
        let xs: Vec<_> = index
            .definitions
            .iter()
            .filter(|definition| definition.name == "x")
            .collect();
        assert_eq!(xs.len(), 2);
    }
}
