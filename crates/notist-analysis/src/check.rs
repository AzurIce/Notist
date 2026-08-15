use std::collections::HashMap;

use notist_model::{DefaultValue, FunctionSignature, Parameter, Type, builtin_signatures};
use notist_syntax::{
    BinaryOperator, Call, Expression, ExpressionKind, Markup, MarkupItem, Parse,
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
    resolver.resolve_markup(&parse.root);
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
                if let Some(symbol) = self.functions.get(&call.name.value).copied() {
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
            ExpressionKind::Unary { operand, .. } => {
                self.resolve_expression(operand);
            }
            ExpressionKind::Import { .. } => {}
            ExpressionKind::Block(statements) => {
                for statement in statements {
                    self.resolve_expression(statement);
                }
            }
            ExpressionKind::Let { value, .. } => {
                self.resolve_expression(value);
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
            ExpressionKind::Lambda { body, .. } => {
                self.resolve_expression(body);
            }
            ExpressionKind::LetFunction(definition) => {
                for parameter in &definition.parameters {
                    if let Some(default) = &parameter.default {
                        self.resolve_expression(default);
                    }
                }
                let mut scope = HashMap::new();
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
            ExpressionKind::None
            | ExpressionKind::Bool(_)
            | ExpressionKind::Int(_)
            | ExpressionKind::Float(_)
            | ExpressionKind::String(_)
            | ExpressionKind::Error => {}
        }
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
        ExpressionKind::None => Some(DefaultValue::None),
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
            if let notist_syntax::MarkupItem::Embedded(embedded) = item {
                let checked = self.type_of_expression(&embedded.expression);
                if let Some(ty) = checked.ty {
                    let insertable = match &ty {
                        Type::Content
                        | Type::String
                        | Type::None
                        | Type::Int
                        | Type::Float
                        | Type::Bool
                        | Type::Inferred => true,
                        _ => false,
                    };
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
                CheckedType::known(Type::None)
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
                let mut result = CheckedType::known(Type::None);
                for statement in statements {
                    let statement_type = self.type_of_expression(statement);
                    result = match (result.ty.clone(), statement_type.ty.clone()) {
                        (None, ty) | (ty, None) => CheckedType { ty },
                        (Some(Type::None), Some(ty)) => CheckedType { ty: Some(ty) },
                        (Some(ty), Some(Type::None)) => CheckedType { ty: Some(ty) },
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
                let ty = annotation.clone().or_else(|| value_type.ty.clone());
                if self.variables.is_empty() {
                    self.variables.push(HashMap::new());
                }
                if let (Some(ty), Some(scope)) = (ty, self.variables.last_mut()) {
                    scope.insert(name.value.clone(), ty);
                }
                CheckedType::known(Type::None)
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
                            (Some(left), Some(right)) if left == right => {
                                CheckedType::known(left)
                            }
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
            ExpressionKind::Import { .. } => CheckedType::known(Type::None),
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
            (
                BinaryOperator::Equal | BinaryOperator::NotEqual,
                left,
                right,
            ) if !matches!(left, Type::Content | Type::Function)
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
            (BinaryOperator::And | BinaryOperator::Or, Type::Bool, Type::Bool) => {
                Some(Type::Bool)
            }
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
        if self.variables.iter().rev().any(|scope| {
            matches!(scope.get(name), Some(Type::Function | Type::Inferred))
        }) {
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
            } else if saw_named
                && matches!(argument.expression.kind, ExpressionKind::Content(_))
            {
                // R05: the trailing Content block is the one positional
                // argument allowed after named arguments.
                let trailing = signature
                    .trailing_content
                    .as_deref()
                    .and_then(|name| {
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
    fn accepts_well_typed_documents() {
        assert!(check("#heading(level=2)[Title]").is_empty());
        assert!(check("#heading[Default level]").is_empty());
        assert!(check("#raw(source=\"code\", lang=\"rust\")").is_empty());
        assert!(check("#details[Quoted [[vault::target]]]").is_empty());
        assert!(check("a#\"string\"#[content]#none z").is_empty());
        assert!(check("#details(body=[ordinary])").is_empty());
        // D0007: `fn(parameters) -> R` is the written function type (R07).
        assert!(check("#let f: fn(x: Int =, trailing body: Content) -> Content = (x: Int) => x * 2").is_empty());
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
}
