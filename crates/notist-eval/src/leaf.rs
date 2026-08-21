//! Stream + Leaf evaluation for the unified Call Reduction model.
//!
//! The document is first a stream of [`StreamNode`] values (`Call | Leaf`).
//! Reduction replaces every call with [`ElementInstance`] leaves, then
//! recursive shaping folds the flat Leaf stream into an [`ElementTree`].

use std::collections::{HashMap, HashSet};

use notist_model::{
    Block, BodyMode, Content, CustomField, Element, ElementInstance, ElementName, ElementNode,
    ElementSchema, ElementValue, Field, FieldValue, InstanceNode, ModuleReference, ShapingKind,
    ShapingRole, StructuredDocument, TableAlignment, TextRange, WikiReference,
};

use crate::call::{CallContent, CallNode};
use crate::type_system::default_to_value;
use crate::{
    BoundArguments, EvalDiagnostic, FunctionContext, FunctionInput, FunctionOutput, FunctionOwner,
    FunctionRegistry, Type, Value,
};

/// The full Stream pipeline result: lowered stream, reduced Leaf stream,
/// and the recursively shaped canonical tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamEvaluation {
    /// Lowering output before reduction. May contain `StreamNode::Call`.
    pub lowered: FlatContent,
    /// Reduction output. Contains only `StreamNode::Leaf`.
    pub reduced: FlatContent,
    /// The recursively shaped canonical tree.
    pub tree: ElementTree,
    /// Lowering, reduction, and parse diagnostics.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// Document-level bindings.
    pub bindings: HashMap<String, Value>,
    /// Side annotation table.
    pub annotations: Vec<crate::AnnotationEntry>,
    /// Module-level attributes.
    pub module_attributes: Vec<notist_syntax::Attributes>,
    /// Whether call reduction failed and `reduced` is therefore empty.
    pub reduction_failed: bool,
}

/// The result of evaluating a document as a shaped Leaf tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeafEvaluation {
    /// The recursively shaped canonical tree.
    pub tree: ElementTree,
    /// Evaluation diagnostics, including reduction errors.
    pub diagnostics: Vec<EvalDiagnostic>,
    /// Root-scope bindings.
    pub bindings: HashMap<String, Value>,
    /// Side annotation table.
    pub annotations: Vec<crate::AnnotationEntry>,
    /// Module-level attributes.
    pub module_attributes: Vec<notist_syntax::Attributes>,
}

impl LeafEvaluation {
    /// Creates a Leaf evaluation from an existing evaluation result.
    pub fn from_evaluation(evaluation: &crate::Evaluation) -> Self {
        let nodes = legacy_content_to_nodes(&evaluation.content);
        Self {
            tree: shape_flat(&nodes),
            diagnostics: evaluation.diagnostics.clone(),
            bindings: evaluation.bindings.clone(),
            annotations: evaluation.annotations.clone(),
            module_attributes: evaluation.module_attributes.clone(),
        }
    }
}

/// One node in a document stream.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamNode {
    /// A call that still needs reduction.
    Call(StreamCall),
    /// A terminal `ElementInstance` leaf.
    Leaf(InstanceNode),
}

/// A sequence of calls and leaves before reduction completes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatContent {
    /// Stream nodes in source order.
    pub nodes: Vec<StreamNode>,
}

impl FlatContent {
    /// Creates an empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a stream from nodes.
    pub fn from_nodes(nodes: impl IntoIterator<Item = StreamNode>) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
        }
    }
}

/// The shaped, canonical Leaf tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementTree {
    /// Top-level roots. No `Call` or `core::parbreak` remains.
    pub roots: Vec<InstanceNode>,
}

/// A call in the stream model.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamCall {
    /// Qualified function name, e.g. `core::details`.
    pub name: String,
    /// Named arguments.
    pub arguments: Vec<StreamArgument>,
    /// Trailing content body.
    pub body: Option<FlatContent>,
    /// Source range of the call.
    pub range: TextRange,
}

impl StreamCall {
    /// Creates an empty call.
    pub fn new(name: impl Into<String>, range: TextRange) -> Self {
        Self {
            name: name.into(),
            arguments: Vec::new(),
            body: None,
            range,
        }
    }

    /// Appends a named argument.
    pub fn argument(mut self, name: impl Into<String>, value: impl Into<StreamValue>) -> Self {
        self.arguments.push(StreamArgument {
            name: name.into(),
            value: value.into(),
        });
        self
    }

    /// Attaches trailing content.
    pub fn with_body(mut self, body: FlatContent) -> Self {
        self.body = Some(body);
        self
    }
}

/// One named call argument.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamArgument {
    /// Parameter name.
    pub name: String,
    /// Argument value or nested stream.
    pub value: StreamValue,
}

/// A call argument value in the stream model.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamValue {
    /// An already-evaluated ordinary value.
    Value(Value),
    /// Content represented as a stream that must be reduced first.
    Stream(FlatContent),
}

impl From<Value> for StreamValue {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

impl From<FlatContent> for StreamValue {
    fn from(value: FlatContent) -> Self {
        Self::Stream(value)
    }
}

/// The principal making a call.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Principal {
    /// The document author / host core.
    Host,
    /// A plugin package.
    Plugin(String),
}

impl From<&FunctionOwner> for Principal {
    fn from(owner: &FunctionOwner) -> Self {
        match owner {
            FunctionOwner::Host => Self::Host,
            FunctionOwner::Plugin(package) => Self::Plugin(package.clone()),
        }
    }
}

/// Declarative capability rules checked at every dispatch.
#[derive(Clone, Debug, Default)]
pub struct CapabilityPolicy {
    rules: HashMap<Principal, HashSet<String>>,
}

impl CapabilityPolicy {
    /// Creates an empty policy. Host calls are always allowed; plugin calls
    /// are denied unless explicitly granted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants `caller` permission to call `callee`.
    pub fn allow(mut self, caller: Principal, callee: impl Into<String>) -> Self {
        self.rules.entry(caller).or_default().insert(callee.into());
        self
    }

    /// Returns whether `caller` may dispatch `callee`.
    pub fn allows(&self, caller: &Principal, callee: &str) -> bool {
        if matches!(caller, Principal::Host) {
            return true;
        }
        let Some(allowed) = self.rules.get(caller) else {
            return false;
        };
        allowed.iter().any(|grant| {
            grant == callee
                || grant
                    .strip_suffix("::*")
                    .is_some_and(|prefix| callee.starts_with(prefix))
        })
    }
}

/// Resource limits for one reduction run.
#[derive(Clone, Debug)]
pub struct ReduceLimits {
    /// Maximum nesting depth for generated call chains.
    pub max_depth: usize,
    /// Maximum number of dispatched calls.
    pub max_calls: usize,
}

impl Default for ReduceLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_calls: 10_000,
        }
    }
}

/// Mutable reduction budget.
#[derive(Clone, Debug)]
pub struct ReduceFrame {
    /// Current call-chain depth.
    pub depth: usize,
    /// Remaining dispatch budget.
    pub remaining_calls: usize,
    /// The principal currently authoring calls.
    pub caller: Principal,
    /// Capability policy applied at dispatch.
    policy: CapabilityPolicy,
}

impl ReduceFrame {
    /// Creates a root frame with the given limits.
    pub fn root(limits: &ReduceLimits) -> Self {
        Self::root_with_policy(limits, CapabilityPolicy::new())
    }

    /// Creates a host root frame carrying a registry capability policy.
    pub fn root_with_policy(limits: &ReduceLimits, policy: CapabilityPolicy) -> Self {
        Self {
            depth: 0,
            remaining_calls: limits.max_calls,
            caller: Principal::Host,
            policy,
        }
    }

    /// Creates a frame for a plugin caller under a capability policy.
    pub fn restricted(limits: &ReduceLimits, caller: Principal, policy: CapabilityPolicy) -> Self {
        Self {
            depth: 0,
            remaining_calls: limits.max_calls,
            caller,
            policy,
        }
    }

    /// Returns a clone of the active capability policy.
    pub fn policy(&self) -> CapabilityPolicy {
        self.policy.clone()
    }

    fn dispatch(
        &mut self,
        limits: &ReduceLimits,
        call: &StreamCall,
    ) -> Result<(), Vec<EvalDiagnostic>> {
        if !self.policy.allows(&self.caller, &call.name) {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "principal `{}` is not allowed to call `{}`",
                    self.caller_label(),
                    call.name
                ),
                range: call.range,
            }]);
        }
        if self.depth >= limits.max_depth {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "call reduction exceeded the maximum depth of {}",
                    limits.max_depth
                ),
                range: call.range,
            }]);
        }
        if self.remaining_calls == 0 {
            return Err(vec![EvalDiagnostic {
                message: format!(
                    "call reduction exceeded the maximum budget of {} calls",
                    limits.max_calls
                ),
                range: call.range,
            }]);
        }
        self.remaining_calls -= 1;
        self.depth += 1;
        Ok(())
    }

    fn caller_label(&self) -> String {
        match &self.caller {
            Principal::Host => "host".into(),
            Principal::Plugin(package) => format!("plugin `{package}`"),
        }
    }
}

/// Declarative shaping rules contributed by core and plugin packages.
#[derive(Clone, Debug, Default)]
pub struct ShapingRegistry {
    schemas: HashMap<ElementName, ElementSchema>,
}

impl ShapingRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the core shaping schema for `core::*` constructors.
    pub fn core() -> &'static Self {
        use std::sync::LazyLock;
        static CORE: LazyLock<ShapingRegistry> = LazyLock::new(|| {
            let mut registry = ShapingRegistry::new();
            let core = |registry: &mut ShapingRegistry, local: &str, kind, body, role| {
                registry.insert(ElementSchema::new(
                    ElementName::core(local),
                    kind,
                    body,
                    role,
                ));
            };
            core(
                &mut registry,
                "text",
                ShapingKind::Inline,
                BodyMode::None,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "reference",
                ShapingKind::Inline,
                BodyMode::None,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "parbreak",
                ShapingKind::Separator,
                BodyMode::None,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "heading",
                ShapingKind::Block,
                BodyMode::Inline,
                ShapingRole::Heading,
            );
            core(
                &mut registry,
                "rule",
                ShapingKind::Block,
                BodyMode::None,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "item",
                ShapingKind::Block,
                BodyMode::Flow,
                ShapingRole::Item,
            );
            core(
                &mut registry,
                "table-cell",
                ShapingKind::Block,
                BodyMode::Flow,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "table",
                ShapingKind::Block,
                BodyMode::Cells,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "figure",
                ShapingKind::Block,
                BodyMode::Flow,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "callout",
                ShapingKind::Block,
                BodyMode::Flow,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "details",
                ShapingKind::Block,
                BodyMode::Flow,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "raw",
                // Raw carries its own `block` field; inline raw participates
                // in the surrounding paragraph while fenced raw stands alone.
                ShapingKind::Unspecified,
                BodyMode::None,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "strong",
                ShapingKind::Inline,
                BodyMode::Inline,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "emph",
                ShapingKind::Inline,
                BodyMode::Inline,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "underline",
                ShapingKind::Inline,
                BodyMode::Inline,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "strike",
                ShapingKind::Inline,
                BodyMode::Inline,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "paragraph",
                ShapingKind::Block,
                BodyMode::Shaped,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "list",
                ShapingKind::Block,
                BodyMode::Shaped,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "section",
                ShapingKind::Block,
                BodyMode::Shaped,
                ShapingRole::None,
            );
            core(
                &mut registry,
                "unresolved-call",
                ShapingKind::Unspecified,
                BodyMode::Flow,
                ShapingRole::None,
            );
            registry
        });
        &CORE
    }

    /// Registers or replaces one schema entry.
    pub fn insert(&mut self, schema: ElementSchema) -> Option<ElementSchema> {
        self.schemas.insert(schema.name.clone(), schema)
    }

    /// Looks up a package-specific schema entry, excluding core fallback.
    pub fn get(&self, name: &ElementName) -> Option<&ElementSchema> {
        self.schemas.get(name)
    }

    /// Resolves the effective schema: package entries override core defaults.
    pub fn effective(&self, name: &ElementName) -> Option<&ElementSchema> {
        self.schemas
            .get(name)
            .or_else(|| ShapingRegistry::core().get(name))
    }

    /// Resolves the shaping kind, falling back to `ElementInstance.block`.
    pub fn kind_of(&self, instance: &ElementInstance) -> ShapingKind {
        match self.effective(&instance.name).map(|schema| schema.kind) {
            Some(ShapingKind::Unspecified) | None => {
                if instance.block {
                    ShapingKind::Block
                } else {
                    ShapingKind::Inline
                }
            }
            Some(kind) => kind,
        }
    }

    /// Resolves the body mode, falling back to `Flow`.
    pub fn body_mode_of(&self, instance: &ElementInstance) -> BodyMode {
        self.effective(&instance.name)
            .map_or(BodyMode::Flow, |schema| schema.body_mode)
    }

    /// Resolves the shaping role.
    pub fn role_of(&self, instance: &ElementInstance) -> ShapingRole {
        self.effective(&instance.name)
            .map_or(ShapingRole::None, |schema| schema.role)
    }
}

/// Reduces a flat stream to Leaf nodes.
pub fn reduce_flat(
    content: &FlatContent,
    registry: &FunctionRegistry,
    limits: &ReduceLimits,
    frame: &mut ReduceFrame,
) -> Result<Vec<InstanceNode>, Vec<EvalDiagnostic>> {
    let (output, diagnostics) = reduce_flat_recovering(content, registry, limits, frame);
    if diagnostics.is_empty() {
        Ok(output)
    } else {
        Err(diagnostics)
    }
}

/// Reduces a flat stream while preserving successful siblings and children.
///
/// Top-level calls that fail produce diagnostics and are skipped instead of
/// aborting the entire stream (D0002 error recovery). Nested reduction inside
/// one call still follows the all-or-nothing signature-binding contract.
pub fn reduce_flat_recovering(
    content: &FlatContent,
    registry: &FunctionRegistry,
    limits: &ReduceLimits,
    frame: &mut ReduceFrame,
) -> (Vec<InstanceNode>, Vec<EvalDiagnostic>) {
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    for node in &content.nodes {
        match node {
            StreamNode::Leaf(leaf) => output.push(leaf.clone()),
            StreamNode::Call(call) => match reduce_call(call, registry, limits, frame) {
                Ok(nodes) => output.extend(nodes),
                Err(mut errors) => diagnostics.append(&mut errors),
            },
        }
    }
    (output, diagnostics)
}

/// Reduces a single stream call to Leaf nodes.
pub fn reduce_call(
    call: &StreamCall,
    registry: &FunctionRegistry,
    limits: &ReduceLimits,
    frame: &mut ReduceFrame,
) -> Result<Vec<InstanceNode>, Vec<EvalDiagnostic>> {
    frame.dispatch(limits, call)?;
    let result = reduce_call_inner(call, registry, limits, frame);
    frame.depth -= 1;
    result
}

fn reduce_call_inner(
    call: &StreamCall,
    registry: &FunctionRegistry,
    limits: &ReduceLimits,
    frame: &mut ReduceFrame,
) -> Result<Vec<InstanceNode>, Vec<EvalDiagnostic>> {
    let function = registry.get(&call.name).ok_or_else(|| {
        vec![EvalDiagnostic {
            message: format!("unknown function `{}`", call.name),
            range: call.range,
        }]
    })?;
    let signature = function.signature();
    let mut diagnostics = Vec::new();

    let mut provided: HashMap<String, StreamValue> = HashMap::new();
    for argument in &call.arguments {
        if provided.contains_key(&argument.name) {
            diagnostics.push(EvalDiagnostic {
                message: format!("argument `{}` was provided more than once", argument.name),
                range: call.range,
            });
            continue;
        }
        if !signature
            .parameters
            .iter()
            .any(|parameter| parameter.name == argument.name)
        {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "unknown argument `{}` for function `{}`",
                    argument.name, call.name
                ),
                range: call.range,
            });
            continue;
        }
        provided.insert(argument.name.clone(), argument.value.clone());
    }

    if let Some(body) = &call.body {
        if let Some(trailing_name) = signature.trailing_content.as_deref() {
            let nodes = reduce_flat(body, registry, limits, frame)?;
            let content = instances_to_legacy_content(&nodes).ok_or_else(|| {
                vec![EvalDiagnostic {
                    message: format!(
                        "trailing content for `{}` contains leaves that cannot be lowered",
                        call.name
                    ),
                    range: call.range,
                }]
            })?;
            provided.insert(
                trailing_name.to_owned(),
                StreamValue::Value(Value::Content(content)),
            );
        } else {
            diagnostics.push(EvalDiagnostic {
                message: format!("function `{}` does not accept trailing content", call.name),
                range: call.range,
            });
        }
    }

    let mut values = bind_validated_arguments(
        &signature,
        &provided,
        registry,
        limits,
        frame,
        &call.name,
        call.range,
        &mut diagnostics,
    );
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let arguments = BoundArguments::from_values(std::mem::take(&mut values));
    let context = FunctionContext {
        registry,
        depth: frame.depth,
    };
    let input = FunctionInput {
        name: &call.name,
        arguments,
        range: call.range,
    };
    let owner = function.owner();
    match function.call(&context, input) {
        Ok(FunctionOutput::Content(content)) => Ok(legacy_content_to_nodes(&content)),
        Ok(FunctionOutput::Value(Value::Content(content))) => Ok(legacy_content_to_nodes(&content)),
        Ok(FunctionOutput::Calls(calls)) => {
            let stream = legacy_calls_to_stream(&calls);
            let saved_caller = frame.caller.clone();
            frame.caller = Principal::from(&owner);
            let reduced = reduce_flat(&stream, registry, limits, frame);
            frame.caller = saved_caller;
            reduced
        }
        Ok(FunctionOutput::Value(value)) => Err(vec![EvalDiagnostic {
            message: format!(
                "function `{}` returned {}, expected Content",
                call.name,
                value.ty()
            ),
            range: call.range,
        }]),
        Err(errors) => Err(errors),
    }
}

#[allow(clippy::too_many_arguments)]
fn bind_validated_arguments(
    signature: &notist_model::FunctionSignature,
    provided: &HashMap<String, StreamValue>,
    registry: &FunctionRegistry,
    limits: &ReduceLimits,
    frame: &mut ReduceFrame,
    call_name: &str,
    range: TextRange,
    diagnostics: &mut Vec<EvalDiagnostic>,
) -> HashMap<String, Value> {
    let mut values = HashMap::new();
    for parameter in &signature.parameters {
        let Some(argument) = provided.get(&parameter.name) else {
            if let Some(default) = &parameter.default {
                values.insert(parameter.name.clone(), default_to_value(default));
            } else {
                diagnostics.push(EvalDiagnostic {
                    message: format!(
                        "missing argument `{}` for function `{call_name}`",
                        parameter.name
                    ),
                    range,
                });
            }
            continue;
        };

        let mut value = match argument {
            StreamValue::Value(value) => value.clone(),
            StreamValue::Stream(stream) => {
                let nodes = match reduce_flat(stream, registry, limits, frame) {
                    Ok(nodes) => nodes,
                    Err(mut errors) => {
                        diagnostics.append(&mut errors);
                        continue;
                    }
                };
                let Some(content) = instances_to_legacy_content(&nodes) else {
                    diagnostics.push(EvalDiagnostic {
                        message: format!(
                            "content argument `{}` contains leaves that cannot be lowered",
                            parameter.name
                        ),
                        range,
                    });
                    continue;
                };
                Value::Content(content)
            }
        };

        if !parameter.ty.accepts(&value.ty()) {
            diagnostics.push(EvalDiagnostic {
                message: format!(
                    "type mismatch for argument `{}`: expected {}, found {}",
                    parameter.name,
                    parameter.ty,
                    value.ty()
                ),
                range,
            });
            continue;
        }
        if parameter.ty == Type::Float
            && let Value::Int(integer) = value
        {
            value = Value::Float(integer as f64);
        }
        values.insert(parameter.name.clone(), value);
    }
    values
}

/// Converts the legacy `CallContent` used by native functions into the new stream IR.
pub fn legacy_calls_to_stream(calls: &CallContent) -> FlatContent {
    let mut nodes = Vec::new();
    for node in &calls.nodes {
        match node {
            CallNode::Call(call) => nodes.push(StreamNode::Call(StreamCall {
                name: call.name.clone(),
                arguments: call
                    .arguments
                    .iter()
                    .map(|argument| StreamArgument {
                        name: argument.name.clone(),
                        value: StreamValue::Value(argument.value.clone()),
                    })
                    .collect(),
                body: call.body.as_ref().map(legacy_calls_to_stream),
                range: call.range,
            })),
            CallNode::Element(element) => {
                nodes.push(StreamNode::Leaf(legacy_node_to_instance(element)));
            }
        }
    }
    FlatContent { nodes }
}

/// Converts legacy evaluated content into the unified Leaf representation.
pub fn legacy_content_to_nodes(content: &Content) -> Vec<InstanceNode> {
    content
        .elements
        .iter()
        .map(legacy_node_to_instance)
        .collect()
}

fn legacy_node_to_instance(node: &ElementNode) -> InstanceNode {
    InstanceNode {
        instance: legacy_element_to_instance(&node.element),
        range: node.range,
    }
}

fn legacy_element_to_instance(element: &Element) -> ElementInstance {
    let body_nodes = |content: &Content| {
        content
            .elements
            .iter()
            .map(legacy_node_to_instance)
            .collect()
    };
    match element {
        Element::Text(text) => ElementInstance::text(text.clone()),
        Element::Parbreak => ElementInstance::parbreak(),
        Element::Reference(reference) => {
            ElementInstance::new(ElementName::core("reference"), false)
                .with_field("url", FieldValue::String(format_wiki_reference(reference)))
        }
        Element::Paragraph(content) => ElementInstance::new(ElementName::core("paragraph"), true)
            .with_body(body_nodes(content)),
        Element::Strong(content) => {
            ElementInstance::new(ElementName::core("strong"), false).with_body(body_nodes(content))
        }
        Element::Emph(content) => {
            ElementInstance::new(ElementName::core("emph"), false).with_body(body_nodes(content))
        }
        Element::Strike(content) => {
            ElementInstance::new(ElementName::core("strike"), false).with_body(body_nodes(content))
        }
        Element::Underline(content) => ElementInstance::new(ElementName::core("underline"), false)
            .with_body(body_nodes(content)),
        Element::Rule => ElementInstance::new(ElementName::core("rule"), true),
        Element::Heading { level, body } => {
            ElementInstance::new(ElementName::core("heading"), true)
                .with_field("level", FieldValue::Int(*level as i64))
                .with_body(body_nodes(body))
        }
        Element::ListItem(content) => ElementInstance::new(ElementName::core("item"), true)
            .with_field("ordered", FieldValue::Bool(false))
            .with_body(body_nodes(content)),
        Element::List { ordered, items } => ElementInstance::new(ElementName::core("list"), true)
            .with_field("ordered", FieldValue::Bool(*ordered))
            .with_body(items.iter().map(legacy_node_to_instance).collect()),
        Element::EnumItem { value, body } => {
            let instance = ElementInstance::new(ElementName::core("item"), true)
                .with_field("ordered", FieldValue::Bool(true))
                .with_body(body_nodes(body));
            match value {
                Some(value) => instance.with_field("value", FieldValue::Int(*value as i64)),
                None => instance,
            }
        }
        Element::TableCell {
            body,
            colspan,
            rowspan,
        } => ElementInstance::new(ElementName::core("table-cell"), true)
            .with_field("colspan", FieldValue::Int(*colspan as i64))
            .with_field("rowspan", FieldValue::Int(*rowspan as i64))
            .with_body(body_nodes(body)),
        Element::Table {
            columns,
            header,
            alignments,
            cells,
        } => {
            let align = alignments
                .iter()
                .map(TableAlignment::to_field)
                .collect::<Vec<_>>()
                .join(",");
            ElementInstance::new(ElementName::core("table"), true)
                .with_field("columns", FieldValue::Int(*columns as i64))
                .with_field("header", FieldValue::Bool(*header))
                .with_field("align", FieldValue::String(align))
                .with_body(cells.iter().map(legacy_node_to_instance).collect())
        }
        Element::Figure {
            body,
            kind,
            supplement,
            caption,
        } => ElementInstance::new(ElementName::core("figure"), true)
            .with_field("kind", FieldValue::String(kind.clone()))
            .with_optional_content_field("supplement", supplement)
            .with_optional_content_field("caption", caption)
            .with_body(body_nodes(body)),
        Element::Callout { kind, title, body } => {
            ElementInstance::new(ElementName::core("callout"), true)
                .with_field("kind", FieldValue::String(kind.clone()))
                .with_optional_content_field("title", title)
                .with_body(body_nodes(body))
        }
        Element::Details {
            summary,
            open,
            body,
        } => ElementInstance::new(ElementName::core("details"), true)
            .with_optional_content_field("summary", summary)
            .with_field("open", FieldValue::Bool(*open))
            .with_body(body_nodes(body)),
        Element::Raw {
            text,
            block,
            language,
        } => ElementInstance::new(ElementName::core("raw"), *block)
            .with_field("source", FieldValue::String(text.clone()))
            .with_field("lang", optional_string(language.as_deref()))
            .with_field("block", FieldValue::Bool(*block)),
        Element::Custom {
            name,
            body,
            block,
            fields,
        } => ElementInstance::new(ElementName::parse(name), *block)
            .with_fields(fields.iter().map(|field| Field {
                name: field.name.clone(),
                value: element_value_to_field_value(&field.value),
            }))
            .with_body(body_nodes(body)),
        Element::UnresolvedCall {
            name,
            arguments,
            trailing,
            block,
        } => ElementInstance::new(ElementName::core("unresolved-call"), *block)
            .with_field("name", FieldValue::String(name.clone()))
            .with_field(
                "arguments",
                arguments
                    .clone()
                    .map_or(FieldValue::None, FieldValue::String),
            )
            .with_body(trailing.as_ref().map_or_else(Vec::new, body_nodes)),
    }
}

/// Converts a shaped [`ElementTree`] into the legacy structured document view.
///
/// This is a compatibility projection for hosts that still consume
/// `StructuredDocument`. New consumers should read `ElementTree` directly.
pub fn element_tree_to_document(tree: &ElementTree) -> Option<StructuredDocument> {
    let blocks = tree
        .roots
        .iter()
        .map(instance_node_to_block)
        .collect::<Option<Vec<_>>>()?;
    Some(StructuredDocument { blocks })
}

fn instance_node_to_block(node: &InstanceNode) -> Option<Block> {
    if node.instance.is_core("section") {
        let level = match node.instance.field("level")? {
            FieldValue::Int(level) => *level as u8,
            _ => return None,
        };
        let mut body = node.instance.body.iter();
        let heading = body.next()?;
        let heading = ElementNode {
            element: instance_to_legacy_element(&heading.instance)?,
            range: heading.range,
        };
        let body = body
            .map(instance_node_to_block)
            .collect::<Option<Vec<_>>>()?;
        return Some(Block::Section {
            level,
            heading,
            body,
        });
    }
    Some(Block::Element(ElementNode {
        element: instance_to_legacy_element(&node.instance)?,
        range: node.range,
    }))
}

/// Converts unified Leaf nodes back into legacy evaluated content.
///
/// `core::section` has no legacy element representation and therefore makes
/// the conversion fail; shaped trees should be converted with
/// [`element_tree_to_document`] instead.
pub fn instances_to_legacy_content(nodes: &[InstanceNode]) -> Option<Content> {
    let mut content = Content::new();
    for node in nodes {
        let element = instance_to_legacy_element(&node.instance)?;
        content.elements.push(ElementNode {
            element,
            range: node.range,
        });
    }
    Some(content)
}

/// Converts one canonical Leaf node into the legacy element projection.
///
/// This is the per-node compatibility bridge used by target renderers while
/// they migrate to `ElementInstance`; it does not project whole sections.
pub fn instance_node_to_legacy(node: &InstanceNode) -> Option<ElementNode> {
    Some(ElementNode {
        element: instance_to_legacy_element(&node.instance)?,
        range: node.range,
    })
}

fn instance_to_legacy_element(instance: &ElementInstance) -> Option<Element> {
    let body = || instances_to_legacy_content(&instance.body);
    // Legacy block containers store unshaped flow content directly. In the
    // canonical tree that content has already been grouped into paragraphs,
    // so unwrap paragraph wrappers when projecting back for legacy consumers.
    let flat_body = || {
        Some(Content {
            elements: nodes_to_legacy_flow_content(&instance.body)?,
        })
    };
    let field = |name: &str| instance.field(name).cloned();
    let string = |name: &str| match field(name)? {
        FieldValue::String(value) => Some(value),
        _ => None,
    };
    let int = |name: &str| match field(name)? {
        FieldValue::Int(value) => Some(value),
        _ => None,
    };
    let bool = |name: &str| match field(name)? {
        FieldValue::Bool(value) => Some(value),
        _ => None,
    };
    let optional_string = |name: &str| match field(name) {
        None | Some(FieldValue::None) => Some(None),
        Some(FieldValue::String(value)) => Some(Some(value)),
        _ => None,
    };
    let optional_content = |name: &str| match field(name) {
        None | Some(FieldValue::None) => Some(None),
        Some(FieldValue::Content(nodes)) => {
            let content = instances_to_legacy_content(&nodes)?;
            Some(Some(content))
        }
        _ => None,
    };

    let Some(local) = instance.name.core_local() else {
        return Some(Element::Custom {
            name: instance.name.to_string(),
            body: flat_body()?,
            block: instance.block,
            fields: instance
                .fields
                .iter()
                .filter_map(|field| {
                    Some(CustomField {
                        name: field.name.clone(),
                        value: field_value_to_element_value(&field.value)?,
                    })
                })
                .collect(),
        });
    };

    let element = match local {
        "text" => Element::Text(string("text")?),
        "parbreak" => Element::Parbreak,
        "reference" => {
            Element::Reference(notist_syntax::parse_wiki_reference(&string("url")?).ok()?)
        }
        "paragraph" => Element::Paragraph(body()?),
        "strong" => Element::Strong(body()?),
        "emph" => Element::Emph(body()?),
        "strike" => Element::Strike(body()?),
        "underline" => Element::Underline(body()?),
        "rule" => Element::Rule,
        "heading" => Element::Heading {
            level: int("level").unwrap_or(1) as u8,
            body: body()?,
        },
        "item" => {
            if bool("ordered").unwrap_or(false) {
                Element::EnumItem {
                    value: int("value").map(|value| value as u32),
                    body: flat_body()?,
                }
            } else {
                Element::ListItem(flat_body()?)
            }
        }
        "list" => Element::List {
            ordered: bool("ordered").unwrap_or(false),
            items: nodes_to_legacy_element_nodes(&instance.body)?,
        },
        "table-cell" => Element::TableCell {
            colspan: int("colspan").unwrap_or(1) as u16,
            rowspan: int("rowspan").unwrap_or(1) as u16,
            body: flat_body()?,
        },
        "table" => Element::Table {
            columns: int("columns").unwrap_or(1) as u16,
            header: bool("header").unwrap_or(false),
            alignments: parse_alignments(&optional_string("align")?.unwrap_or_default()),
            cells: nodes_to_legacy_element_nodes(&instance.body)?,
        },
        "figure" => Element::Figure {
            kind: string("kind").unwrap_or_else(|| "figure".into()),
            supplement: optional_content("supplement")?,
            caption: optional_content("caption")?,
            body: flat_body()?,
        },
        "callout" => Element::Callout {
            kind: string("kind").unwrap_or_else(|| "note".into()),
            title: optional_content("title")?,
            body: flat_body()?,
        },
        "details" => Element::Details {
            summary: optional_content("summary")?,
            open: bool("open").unwrap_or(false),
            body: flat_body()?,
        },
        "raw" => Element::Raw {
            text: string("source")?,
            block: bool("block").unwrap_or(false),
            language: optional_string("lang")?,
        },
        "unresolved-call" => Element::UnresolvedCall {
            name: string("name")?,
            arguments: optional_string("arguments")?,
            trailing: Some(body()?).filter(|content| !content.is_empty()),
            block: instance.block,
        },
        _ => return None,
    };
    Some(element)
}

fn nodes_to_legacy_element_nodes(nodes: &[InstanceNode]) -> Option<Vec<ElementNode>> {
    nodes
        .iter()
        .map(|node| {
            Some(ElementNode {
                element: instance_to_legacy_element(&node.instance)?,
                range: node.range,
            })
        })
        .collect()
}

/// Converts a shaped body to legacy flow content by unwrapping paragraph
/// wrappers. Canonical shaping groups inline runs into `core::paragraph`;
/// legacy block containers expect those runs directly in their body.
fn nodes_to_legacy_flow_content(nodes: &[InstanceNode]) -> Option<Vec<ElementNode>> {
    let mut output = Vec::new();
    for node in nodes {
        if node.instance.is_core("paragraph") {
            output.extend(nodes_to_legacy_element_nodes(&node.instance.body)?);
        } else {
            output.push(ElementNode {
                element: instance_to_legacy_element(&node.instance)?,
                range: node.range,
            });
        }
    }
    Some(output)
}

fn field_value_to_element_value(value: &FieldValue) -> Option<ElementValue> {
    let converted = match value {
        FieldValue::None => ElementValue::None,
        FieldValue::Bool(value) => ElementValue::Bool(*value),
        FieldValue::Int(value) => ElementValue::Int(*value),
        FieldValue::Float(value) => ElementValue::Float(*value),
        FieldValue::String(value) => ElementValue::String(value.clone()),
        FieldValue::Content(nodes) => ElementValue::Content(instances_to_legacy_content(nodes)?),
        FieldValue::Array(values) => ElementValue::Array(
            values
                .iter()
                .map(field_value_to_element_value)
                .collect::<Option<Vec<_>>>()?,
        ),
    };
    Some(converted)
}

fn element_value_to_field_value(value: &ElementValue) -> FieldValue {
    match value {
        ElementValue::None => FieldValue::None,
        ElementValue::Bool(value) => FieldValue::Bool(*value),
        ElementValue::Int(value) => FieldValue::Int(*value),
        ElementValue::Float(value) => FieldValue::Float(*value),
        ElementValue::String(value) => FieldValue::String(value.clone()),
        ElementValue::Content(content) => FieldValue::Content(legacy_content_to_nodes(content)),
        ElementValue::Array(values) => {
            FieldValue::Array(values.iter().map(element_value_to_field_value).collect())
        }
    }
}

/// Recursively shapes a flat Leaf stream into the canonical tree using the
/// built-in core shaping schema.
pub fn shape_flat(nodes: &[InstanceNode]) -> ElementTree {
    shape_flat_with(nodes, ShapingRegistry::core())
}

/// Recursively shapes a flat Leaf stream using a caller-provided schema registry.
pub fn shape_flat_with(nodes: &[InstanceNode], registry: &ShapingRegistry) -> ElementTree {
    ElementTree {
        roots: shape_flow(nodes, registry),
    }
}

fn shape_instance(node: &InstanceNode, registry: &ShapingRegistry) -> InstanceNode {
    let instance = shape_element(&node.instance, registry);
    InstanceNode {
        instance,
        range: node.range,
    }
}

fn shape_element(instance: &ElementInstance, registry: &ShapingRegistry) -> ElementInstance {
    let body = match registry.body_mode_of(instance) {
        BodyMode::Shaped | BodyMode::None => instance.body.clone(),
        BodyMode::Inline => shape_inline(&instance.body, registry),
        BodyMode::Flow => shape_flow(&instance.body, registry),
        BodyMode::Cells => instance.body.clone(),
    };
    ElementInstance {
        fields: shape_fields(&instance.fields, registry),
        body,
        ..instance.clone()
    }
}

fn shape_fields(fields: &[Field], registry: &ShapingRegistry) -> Vec<Field> {
    fields
        .iter()
        .map(|field| Field {
            name: field.name.clone(),
            value: match &field.value {
                FieldValue::Content(nodes) => FieldValue::Content(shape_inline(nodes, registry)),
                FieldValue::Array(values) => FieldValue::Array(
                    values
                        .iter()
                        .map(|value| shape_field_value(value, registry))
                        .collect(),
                ),
                value => value.clone(),
            },
        })
        .collect()
}

fn shape_field_value(value: &FieldValue, registry: &ShapingRegistry) -> FieldValue {
    match value {
        FieldValue::Content(nodes) => FieldValue::Content(shape_inline(nodes, registry)),
        FieldValue::Array(values) => FieldValue::Array(
            values
                .iter()
                .map(|value| shape_field_value(value, registry))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn shape_inline(nodes: &[InstanceNode], registry: &ShapingRegistry) -> Vec<InstanceNode> {
    nodes
        .iter()
        .filter(|node| registry.kind_of(&node.instance) != ShapingKind::Separator)
        .cloned()
        .collect()
}

fn shape_flow(nodes: &[InstanceNode], registry: &ShapingRegistry) -> Vec<InstanceNode> {
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();

    for node in nodes {
        match registry.kind_of(&node.instance) {
            ShapingKind::Separator => flush_paragraph(&mut paragraph, &mut blocks),
            ShapingKind::Inline | ShapingKind::Unspecified if !node.instance.block => {
                if paragraph.is_empty() && is_framing_whitespace(node) {
                    continue;
                }
                paragraph.push(node.clone());
            }
            ShapingKind::Inline | ShapingKind::Block | ShapingKind::Unspecified => {
                if registry.role_of(&node.instance) == ShapingRole::Item {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    push_list_item(&mut blocks, node, registry);
                } else {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    blocks.push(shape_instance(node, registry));
                }
            }
        }
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    group_sections(blocks, registry)
}

fn is_framing_whitespace(node: &InstanceNode) -> bool {
    node.instance.is_core("text")
        && node.instance.field("text").is_some_and(
            |value| matches!(value, FieldValue::String(text) if text.trim().is_empty()),
        )
}

fn flush_paragraph(paragraph: &mut Vec<InstanceNode>, blocks: &mut Vec<InstanceNode>) {
    if paragraph.is_empty() {
        return;
    }
    let range = TextRange::new(
        paragraph.first().unwrap().range.start,
        paragraph.last().unwrap().range.end,
    );
    let instance = ElementInstance::new(ElementName::core("paragraph"), true)
        .with_body(std::mem::take(paragraph));
    blocks.push(InstanceNode { instance, range });
}

fn push_list_item(blocks: &mut Vec<InstanceNode>, node: &InstanceNode, registry: &ShapingRegistry) {
    let ordered = node
        .instance
        .field("ordered")
        .is_some_and(|value| matches!(value, FieldValue::Bool(true)));
    if let Some(last) = blocks.last_mut()
        && last.instance.is_core("list")
        && last.instance.field("ordered").is_some_and(
            |value| matches!(value, FieldValue::Bool(ordered_value) if *ordered_value == ordered),
        )
    {
        last.range.end = node.range.end;
        last.instance.body.push(shape_instance(node, registry));
        return;
    }

    let range = node.range;
    let instance = ElementInstance::new(ElementName::core("list"), true)
        .with_field("ordered", FieldValue::Bool(ordered))
        .with_child(shape_instance(node, registry));
    blocks.push(InstanceNode { instance, range });
}

fn group_sections(blocks: Vec<InstanceNode>, registry: &ShapingRegistry) -> Vec<InstanceNode> {
    #[derive(Clone)]
    struct OpenSection {
        level: i64,
        heading: InstanceNode,
        body: Vec<InstanceNode>,
    }

    fn push_section(
        output: &mut Vec<InstanceNode>,
        open: &mut [OpenSection],
        section: OpenSection,
    ) {
        let range = TextRange::new(
            section.heading.range.start,
            section
                .body
                .last()
                .map_or(section.heading.range.end, |node| node.range.end),
        );
        let mut body = Vec::with_capacity(section.body.len() + 1);
        body.push(section.heading);
        body.extend(section.body);
        let instance = ElementInstance::new(ElementName::core("section"), true)
            .with_field("level", FieldValue::Int(section.level))
            .with_body(body);
        let section_node = InstanceNode { instance, range };
        match open.last_mut() {
            Some(parent) => parent.body.push(section_node),
            None => output.push(section_node),
        }
    }

    let mut output = Vec::new();
    let mut open: Vec<OpenSection> = Vec::new();
    for block in blocks {
        let heading_level = (registry.role_of(&block.instance) == ShapingRole::Heading)
            .then(|| {
                block.instance.field("level").and_then(|value| match value {
                    FieldValue::Int(level) => Some(*level),
                    _ => None,
                })
            })
            .flatten();
        if let Some(level) = heading_level {
            while open.last().is_some_and(|section| section.level >= level) {
                let section = open.pop().unwrap();
                push_section(&mut output, &mut open, section);
            }
            open.push(OpenSection {
                level,
                heading: block,
                body: Vec::new(),
            });
        } else if let Some(parent) = open.last_mut() {
            parent.body.push(block);
        } else {
            output.push(block);
        }
    }
    while let Some(section) = open.pop() {
        push_section(&mut output, &mut open, section);
    }
    output
}

trait ElementInstanceExt {
    fn with_body(self, body: Vec<InstanceNode>) -> Self;
    fn with_fields(self, fields: impl IntoIterator<Item = Field>) -> Self;
    fn with_optional_content_field(self, name: &str, content: &Option<Content>) -> Self;
}

impl ElementInstanceExt for ElementInstance {
    fn with_body(mut self, body: Vec<InstanceNode>) -> Self {
        self.body = body;
        self
    }

    fn with_fields(mut self, fields: impl IntoIterator<Item = Field>) -> Self {
        self.fields.extend(fields);
        self
    }

    fn with_optional_content_field(mut self, name: &str, content: &Option<Content>) -> Self {
        if let Some(content) = content {
            self.fields.push(Field::new(
                name,
                FieldValue::Content(legacy_content_to_nodes(content)),
            ));
        }
        self
    }
}

fn optional_string(value: Option<&str>) -> FieldValue {
    match value {
        Some(value) => FieldValue::String(value.to_owned()),
        None => FieldValue::None,
    }
}

trait TableAlignmentField {
    fn to_field(&self) -> &'static str;
}

impl TableAlignmentField for TableAlignment {
    fn to_field(&self) -> &'static str {
        match self {
            TableAlignment::Default => "default",
            TableAlignment::Left => "left",
            TableAlignment::Center => "center",
            TableAlignment::Right => "right",
        }
    }
}

fn parse_alignments(source: &str) -> Vec<TableAlignment> {
    source
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| match segment {
            "left" => TableAlignment::Left,
            "center" => TableAlignment::Center,
            "right" => TableAlignment::Right,
            _ => TableAlignment::Default,
        })
        .collect()
}

pub(crate) fn format_wiki_reference(reference: &WikiReference) -> String {
    let module = match &reference.module {
        ModuleReference::Absolute(segments) => format!("vault::{}", segments.join("::")),
        ModuleReference::Relative(segments) => {
            if segments.is_empty() {
                String::new()
            } else {
                format!("self::{}", segments.join("::"))
            }
        }
        ModuleReference::Parent { levels, remainder } => {
            let mut path = vec!["super"; *levels]
                .into_iter()
                .chain(remainder.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("::");
            if path.is_empty() {
                path = "super".into();
            }
            path
        }
        ModuleReference::External(url) => url.clone(),
    };
    match &reference.label {
        Some(label) => format!("{module}#{label}"),
        None => module,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Evaluator;

    #[test]
    fn source_evaluates_to_shaped_leaf_tree() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate("hello\n\nworld");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let nodes = legacy_content_to_nodes(&evaluation.content);
        let tree = shape_flat(&nodes);

        assert_eq!(tree.roots.len(), 2);
        assert!(
            tree.roots
                .iter()
                .all(|node| node.instance.is_core("paragraph"))
        );
        assert_eq!(
            tree.roots[0].instance.body.len(),
            1,
            "first paragraph contains one text leaf"
        );
        assert!(tree.roots[0].instance.body[0].instance.is_core("text"));
    }

    #[test]
    fn stream_reduction_composes_details_raw_and_text() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);

        let summary = FlatContent::from_nodes([StreamNode::Leaf(InstanceNode::synthetic(
            ElementInstance::text("Shader"),
        ))]);
        let body = FlatContent::from_nodes([
            StreamNode::Call(
                StreamCall::new("raw", TextRange::new(0, 0))
                    .argument("source", Value::String("fn main() {}".into()))
                    .argument("lang", Value::String("wgsl".into()))
                    .argument("block", Value::Bool(true)),
            ),
            StreamNode::Leaf(InstanceNode::synthetic(ElementInstance::text("fallback"))),
        ]);
        let call = StreamCall::new("details", TextRange::new(0, 0))
            .argument("summary", StreamValue::Stream(summary))
            .argument("open", Value::Bool(false))
            .with_body(body);

        let nodes = reduce_call(&call, &registry, &limits, &mut frame).unwrap();
        let tree = shape_flat(&nodes);

        assert_eq!(tree.roots.len(), 1);
        assert!(tree.roots[0].instance.is_core("details"));
        assert!(
            tree.roots[0]
                .instance
                .body
                .iter()
                .any(|node| node.instance.is_core("raw"))
        );
        assert!(
            tree.roots[0]
                .instance
                .body
                .iter()
                .any(|node| node.instance.is_core("paragraph")
                    && node.instance.body[0].instance.is_core("text"))
        );
    }

    #[test]
    fn qualified_core_aliases_reduce_in_the_stream_engine() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        let call = StreamCall::new("core::details", TextRange::new(0, 0)).with_body(
            FlatContent::from_nodes([StreamNode::Call(
                StreamCall::new("core::raw", TextRange::new(0, 0))
                    .argument("source", Value::String("fn main() {}".into()))
                    .argument("block", Value::Bool(true)),
            )]),
        );

        let nodes = reduce_call(&call, &registry, &limits, &mut frame).unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].instance.is_core("details"));
        assert!(
            nodes[0]
                .instance
                .body
                .iter()
                .any(|node| node.instance.is_core("raw"))
        );
    }

    #[test]
    fn evaluate_leaf_shapes_sections_recursively() {
        let evaluator = Evaluator::default();
        let leaf_evaluation = evaluator.evaluate_leaf(
            "= Title

Body text",
        );

        assert!(
            leaf_evaluation.diagnostics.is_empty(),
            "{:?}",
            leaf_evaluation.diagnostics
        );
        assert_eq!(leaf_evaluation.tree.roots.len(), 1);
        assert!(leaf_evaluation.tree.roots[0].instance.is_core("section"));
        assert!(
            leaf_evaluation.tree.roots[0].instance.body[0]
                .instance
                .is_core("heading")
        );
        assert!(
            leaf_evaluation.tree.roots[0]
                .instance
                .body
                .iter()
                .any(|node| node.instance.is_core("paragraph"))
        );
    }

    #[test]
    fn stream_pipeline_binds_positional_arguments() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate_stream("#raw(\"fn main() {}\")\n");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Some(StreamNode::Leaf(raw)) =
            evaluation.reduced.nodes.iter().find(
                |node| matches!(node, StreamNode::Leaf(leaf) if leaf.instance.is_core("raw")),
            )
        else {
            panic!("expected raw leaf");
        };
        assert!(raw.instance.is_core("raw"));
    }

    #[test]
    fn stream_pipeline_binds_positional_arguments_past_optional_parameters() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate_stream("#raw(\"fn\", \"rust\", true)");
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        let Some(StreamNode::Leaf(raw)) = evaluation.reduced.nodes.first() else {
            panic!("expected raw leaf, got {:#?}", evaluation.reduced.nodes)
        };
        assert_eq!(
            raw.instance.field("lang"),
            Some(&notist_model::FieldValue::String("rust".into()))
        );
        assert_eq!(
            raw.instance.field("block"),
            Some(&notist_model::FieldValue::Bool(true))
        );
    }

    #[test]
    fn capability_policy_checks_plugin_returned_calls() {
        use crate::{Function, FunctionContext, FunctionInput, FunctionOutput, FunctionSignature};

        struct PluginA;

        impl Function for PluginA {
            fn name(&self) -> &str {
                "a::entry"
            }

            fn signature(&self) -> FunctionSignature {
                FunctionSignature {
                    parameters: Vec::new(),
                    trailing_content: None,
                    result: crate::Type::Content,
                }
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                input: FunctionInput<'_>,
            ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
                Ok(FunctionOutput::calls(crate::call::CallContent {
                    nodes: vec![crate::call::CallNode::Call(crate::call::Call {
                        name: "denied".into(),
                        arguments: Vec::new(),
                        body: None,
                        range: input.range,
                    })],
                }))
            }

            fn owner(&self) -> FunctionOwner {
                FunctionOwner::Plugin("a".into())
            }
        }

        let mut registry = FunctionRegistry::new();
        registry.register(PluginA).unwrap();
        let limits = ReduceLimits::default();
        let policy = CapabilityPolicy::new()
            .allow(Principal::Plugin("a".into()), "a::entry")
            .allow(Principal::Plugin("a".into()), "allowed");
        let mut frame = ReduceFrame::restricted(&limits, Principal::Plugin("a".into()), policy);

        let error = reduce_call(
            &StreamCall::new("a::entry", TextRange::new(0, 0)),
            &registry,
            &limits,
            &mut frame,
        )
        .unwrap_err();
        assert!(
            error
                .iter()
                .any(|diagnostic| diagnostic.message.contains("not allowed to call `denied`")),
            "{error:?}"
        );
    }

    #[test]
    fn shaping_registry_drives_plugin_leaf_body_mode() {
        use notist_model::{ElementName, ElementSchema, ShapingKind, ShapingRole};

        assert_eq!(
            ShapingRegistry::core()
                .get(&ElementName::core("parbreak"))
                .map(|schema| schema.kind),
            Some(ShapingKind::Separator)
        );
        let mut registry = ShapingRegistry::new();
        registry.insert(ElementSchema::new(
            ElementName::plugin("demo", "box"),
            ShapingKind::Block,
            notist_model::BodyMode::Flow,
            ShapingRole::None,
        ));
        let body = vec![
            InstanceNode::synthetic(ElementInstance::text("first")),
            InstanceNode::synthetic(ElementInstance::parbreak()),
            InstanceNode::synthetic(ElementInstance::text("second")),
        ];
        let leaf = InstanceNode::synthetic(
            ElementInstance::new(ElementName::plugin("demo", "box"), true).with_body(body),
        );
        let tree = shape_flat_with(&[leaf], &registry);

        assert_eq!(tree.roots.len(), 1);
        assert!(!tree.roots[0].instance.is_core("box"));
        assert_eq!(tree.roots[0].instance.body.len(), 2);
        assert!(
            tree.roots[0]
                .instance
                .body
                .iter()
                .all(|node| node.instance.is_core("paragraph"))
        );
    }

    #[test]
    fn element_tree_projects_back_to_legacy_structured_document() {
        let source = "= Title\n\nBefore after.\n\n- one\n  - nested\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let evaluator = Evaluator::default();
        let stream = evaluator.evaluate_stream(source);
        assert!(stream.diagnostics.is_empty(), "{:?}", stream.diagnostics);
        let projected = element_tree_to_document(&stream.tree).expect("tree projects to document");
        assert!(matches!(
            projected.blocks.as_slice(),
            [Block::Section { .. }]
        ));
        let Block::Section { body, .. } = &projected.blocks[0] else {
            unreachable!()
        };
        assert!(body.iter().any(|block| matches!(block, Block::Element(node) if matches!(node.element, Element::List { .. }))));
        assert!(body.iter().any(|block| matches!(block, Block::Element(node) if matches!(node.element, Element::Table { .. }))));
    }

    #[test]
    fn stream_pipeline_lowers_list_and_table_sugar_directly() {
        let evaluator = Evaluator::default();
        let list = evaluator.evaluate_stream("- a\n  - b\n- c");
        assert!(list.diagnostics.is_empty(), "{:?}", list.diagnostics);
        assert!(
            list.lowered
                .nodes
                .iter()
                .any(|node| matches!(node, StreamNode::Call(call) if call.name == "item")),
            "{:#?}",
            list.lowered
        );
        assert_eq!(list.tree.roots.len(), 1);
        assert!(list.tree.roots[0].instance.is_core("list"));
        assert!(
            list.tree.roots[0].instance.body[0]
                .instance
                .body
                .iter()
                .any(|node| node.instance.is_core("list"))
        );

        let table = evaluator.evaluate_stream("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(table.diagnostics.is_empty(), "{:?}", table.diagnostics);
        assert!(
            table
                .lowered
                .nodes
                .iter()
                .any(|node| matches!(node, StreamNode::Call(call) if call.name == "table")),
            "{:#?}",
            table.lowered
        );
        assert_eq!(table.tree.roots.len(), 1);
        assert!(table.tree.roots[0].instance.is_core("table"));
    }

    #[test]
    fn stream_pipeline_lowers_calls_before_reducing() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate_stream("= Title\n\nHello");

        assert!(
            evaluation
                .lowered
                .nodes
                .iter()
                .any(|node| matches!(node, StreamNode::Call(call) if call.name == "heading")),
            "{:#?}",
            evaluation.lowered
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert!(evaluation.tree.roots[0].instance.is_core("section"));
    }

    #[test]
    fn stream_pipeline_reduces_explicit_calls() {
        let evaluator = Evaluator::default();
        let evaluation = evaluator.evaluate_stream("#details[hello]");

        assert!(
            evaluation
                .lowered
                .nodes
                .iter()
                .any(|node| matches!(node, StreamNode::Call(call) if call.name == "details")),
            "{:#?}",
            evaluation.lowered
        );
        assert!(
            evaluation.diagnostics.is_empty(),
            "{:?}",
            evaluation.diagnostics
        );
        assert_eq!(evaluation.reduced.nodes.len(), 1);
        let StreamNode::Leaf(details) = &evaluation.reduced.nodes[0] else {
            panic!("expected reduced details leaf");
        };
        assert!(details.instance.is_core("details"));
        assert_eq!(evaluation.tree.roots.len(), 1);
        assert!(evaluation.tree.roots[0].instance.is_core("details"));
    }

    #[test]
    fn reduction_enforces_depth_budget_for_recursive_plugins() {
        use crate::{Function, FunctionContext, FunctionInput, FunctionOutput, FunctionSignature};

        struct Recursive;

        impl Function for Recursive {
            fn name(&self) -> &str {
                "recursive"
            }

            fn signature(&self) -> FunctionSignature {
                FunctionSignature {
                    parameters: Vec::new(),
                    trailing_content: None,
                    result: crate::Type::Content,
                }
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                input: FunctionInput<'_>,
            ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
                Ok(FunctionOutput::calls(crate::call::CallContent {
                    nodes: vec![crate::call::CallNode::Call(crate::call::Call {
                        name: "recursive".into(),
                        arguments: Vec::new(),
                        body: None,
                        range: input.range,
                    })],
                }))
            }
        }

        let mut registry = FunctionRegistry::new();
        registry.register(Recursive).unwrap();
        let limits = ReduceLimits {
            max_depth: 4,
            max_calls: 100,
        };
        let mut frame = ReduceFrame::root(&limits);
        let call = StreamCall::new("recursive", TextRange::new(0, 0));

        let error = reduce_call(&call, &registry, &limits, &mut frame).unwrap_err();
        assert!(
            error
                .iter()
                .any(|diagnostic| diagnostic.message.contains("maximum depth")),
            "{error:?}"
        );
    }

    #[test]
    fn reduction_rejects_unknown_arguments_instead_of_panicking() {
        let registry = FunctionRegistry::with_builtins();
        let limits = ReduceLimits::default();
        let mut frame = ReduceFrame::root(&limits);
        let call = StreamCall::new("heading", TextRange::new(0, 0))
            .argument("level", Value::String("not an int".into()))
            .argument("missing-param", Value::None)
            .with_body(FlatContent::from_nodes([StreamNode::Leaf(
                InstanceNode::synthetic(ElementInstance::text("title")),
            )]));

        let error = reduce_call(&call, &registry, &limits, &mut frame).unwrap_err();
        assert!(error.iter().any(|diagnostic| {
            diagnostic.message.contains("type mismatch")
                || diagnostic.message.contains("unknown argument")
        }));
    }
}
