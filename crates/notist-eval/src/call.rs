//! Uniform call representation for plugin/eval reduction.

use notist_model::{Content, ElementNode, TextRange};

use crate::{EvalDiagnostic, FunctionRegistry, Value};

/// A function call in the uniform reduction model.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub arguments: Vec<Argument>,
    pub body: Option<CallContent>,
    pub range: TextRange,
}

impl Call {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: Vec::new(),
            body: None,
            range: TextRange::new(0, 0),
        }
    }
}

/// One named argument value.
#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    pub name: String,
    pub value: Value,
}

/// A sequence of calls or already-reduced elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallContent {
    pub nodes: Vec<CallNode>,
}

impl CallContent {
    pub fn new() -> Self {
        Self::default()
    }
}

/// One node in a `CallContent`.
#[derive(Clone, Debug, PartialEq)]
pub enum CallNode {
    Call(Call),
    Element(ElementNode),
}

/// Reduces a single call to final Notist content.
pub fn reduce(call: &Call, registry: &FunctionRegistry) -> Result<Content, Vec<EvalDiagnostic>> {
    reduce_content(
        &CallContent {
            nodes: vec![CallNode::Call(call.clone())],
        },
        registry,
    )
}

/// Reduces a sequence of calls to final Notist content.
///
/// This is the compatibility entry point for native code that still returns
/// the legacy `CallContent`. Reduction is delegated to the Stream + Leaf
/// engine, which validates arguments, threads depth/fuel, and lowers the
/// reduced leaves back into legacy `Content`.
pub fn reduce_content(
    content: &CallContent,
    registry: &FunctionRegistry,
) -> Result<Content, Vec<EvalDiagnostic>> {
    reduce_content_as(content, registry)
}

/// Reduces legacy call content (legacy name kept for compatibility).
#[allow(dead_code)]
pub fn reduce_content_as(
    content: &CallContent,
    registry: &FunctionRegistry,
) -> Result<Content, Vec<EvalDiagnostic>> {
    let limits = crate::leaf::ReduceLimits::default();
    let mut frame = crate::leaf::ReduceFrame::root(&limits);
    let stream = crate::leaf::legacy_calls_to_stream(content);
    let nodes = crate::leaf::reduce_flat(&stream, registry, &limits, &mut frame)?;
    crate::leaf::instances_to_legacy_content(&nodes).ok_or_else(|| {
        vec![EvalDiagnostic {
            message: "reduced content contains leaves that cannot be lowered to legacy elements"
                .into(),
            range: content
                .nodes
                .first()
                .map(|node| match node {
                    CallNode::Call(call) => call.range,
                    CallNode::Element(element) => element.range,
                })
                .unwrap_or(notist_model::TextRange::new(0, 0)),
        }]
    })
}

#[cfg(test)]
mod tests {
    use notist_model::{Content, Element, ElementNode, TextRange};

    use super::*;
    use crate::{
        Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
        FunctionSignature, Parameter, Type,
    };

    struct TextFunction;

    impl Function for TextFunction {
        fn name(&self) -> &str {
            "test::text"
        }

        fn signature(&self) -> FunctionSignature {
            FunctionSignature {
                parameters: vec![Parameter {
                    name: "text".into(),
                    ty: Type::String,
                    default: None,
                }],
                trailing_content: None,
                result: Type::Content,
            }
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            let text = input.arguments.string("text").to_owned();
            Ok(FunctionOutput::content(Content::single(
                Element::Text(text),
                input.range,
            )))
        }
    }

    struct WrapperFunction;

    impl Function for WrapperFunction {
        fn name(&self) -> &str {
            "test::wrapper"
        }

        fn signature(&self) -> FunctionSignature {
            FunctionSignature {
                parameters: Vec::new(),
                trailing_content: None,
                result: Type::Content,
            }
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            Ok(FunctionOutput::calls(CallContent {
                nodes: vec![
                    CallNode::Call(Call {
                        name: "test::text".into(),
                        arguments: vec![Argument {
                            name: "text".into(),
                            value: Value::String("hello".into()),
                        }],
                        body: None,
                        range: input.range,
                    }),
                    CallNode::Element(ElementNode {
                        element: Element::Text("world".into()),
                        range: input.range,
                    }),
                ],
            }))
        }
    }

    struct TestShaderFunction;

    impl Function for TestShaderFunction {
        fn name(&self) -> &str {
            "shader"
        }

        fn signature(&self) -> FunctionSignature {
            FunctionSignature {
                parameters: vec![Parameter {
                    name: "source".into(),
                    ty: Type::String,
                    default: None,
                }],
                trailing_content: None,
                result: Type::Content,
            }
        }

        fn call(
            &self,
            _context: &FunctionContext<'_>,
            input: FunctionInput<'_>,
        ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
            let source = input.arguments.string("source").to_owned();
            Ok(FunctionOutput::content(Content::single(
                Element::Custom {
                    name: "shader".into(),
                    body: Content::new(),
                    block: true,
                    fields: vec![notist_model::CustomField {
                        name: "source".into(),
                        value: notist_model::ElementValue::String(source),
                    }],
                },
                input.range,
            )))
        }
    }

    #[test]
    fn reduce_composes_details_raw_and_shader() {
        let mut registry = FunctionRegistry::with_builtins();
        registry.register(TestShaderFunction).unwrap();

        let call = Call {
            name: "details".into(),
            arguments: Vec::new(),
            body: Some(CallContent {
                nodes: vec![
                    CallNode::Call(Call {
                        name: "raw".into(),
                        arguments: vec![
                            Argument {
                                name: "source".into(),
                                value: Value::String("fn main() {}".into()),
                            },
                            Argument {
                                name: "lang".into(),
                                value: Value::String("wgsl".into()),
                            },
                            Argument {
                                name: "block".into(),
                                value: Value::Bool(true),
                            },
                        ],
                        body: None,
                        range: TextRange::new(0, 0),
                    }),
                    CallNode::Call(Call {
                        name: "shader".into(),
                        arguments: vec![Argument {
                            name: "source".into(),
                            value: Value::String("fn mainImage(...) {}".into()),
                        }],
                        body: None,
                        range: TextRange::new(0, 0),
                    }),
                ],
            }),
            range: TextRange::new(0, 0),
        };

        let content = reduce(&call, &registry).unwrap();
        assert_eq!(content.elements.len(), 1);
        let Element::Details { body, .. } = &content.elements[0].element else {
            panic!("expected details element");
        };
        assert!(
            body.elements
                .iter()
                .any(|node| matches!(node.element, Element::Raw { .. }))
        );
        assert!(
            body.elements.iter().any(
                |node| matches!(&node.element, Element::Custom { name, .. } if name == "shader")
            )
        );
    }

    #[test]
    fn reduce_expands_calls_into_content() {
        let mut registry = FunctionRegistry::new();
        registry.register(TextFunction).unwrap();
        registry.register(WrapperFunction).unwrap();

        let call = Call {
            name: "test::wrapper".into(),
            arguments: Vec::new(),
            body: None,
            range: TextRange::new(0, 0),
        };

        let content = reduce(&call, &registry).unwrap();
        let texts: Vec<_> = content
            .elements
            .iter()
            .filter_map(|node| match &node.element {
                Element::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["hello", "world"]);
    }
}
