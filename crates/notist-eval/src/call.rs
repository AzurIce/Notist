//! Uniform call representation for plugin/eval reduction.

use notist_model::{Content, ElementNode, TextRange};

use crate::{
    BoundArguments, EvalDiagnostic, FunctionContext, FunctionInput, FunctionOutput,
    FunctionRegistry, Value,
};

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
    let function = registry.get(&call.name).ok_or_else(|| {
        vec![EvalDiagnostic {
            message: format!("unknown function `{}`", call.name),
            range: call.range,
        }]
    })?;

    let mut values: std::collections::HashMap<String, Value> = call
        .arguments
        .iter()
        .map(|argument| (argument.name.clone(), argument.value.clone()))
        .collect();
    let signature = function.signature();
    if let Some(body) = &call.body {
        let content = reduce_content(body, registry)?;
        if let Some(trailing_name) = signature.trailing_content.as_deref() {
            values.insert(trailing_name.to_owned(), Value::Content(content));
        }
    }
    let arguments = BoundArguments::from_values(values);

    let context = FunctionContext {
        registry,
        depth: 0,
    };
    let input = FunctionInput {
        name: &call.name,
        arguments,
        range: call.range,
    };
    let output = function.call(&context, input)?;

    match output {
        FunctionOutput::Content(content) => Ok(content),
        FunctionOutput::Calls(calls) => reduce_content(&calls, registry),
        FunctionOutput::Value(_) => Err(vec![EvalDiagnostic {
            message: format!("function `{}` did not return Content", call.name),
            range: call.range,
        }]),
    }
}

/// Reduces a sequence of calls to final Notist content.
pub fn reduce_content(
    content: &CallContent,
    registry: &FunctionRegistry,
) -> Result<Content, Vec<EvalDiagnostic>> {
    let mut output = Content::new();
    for node in &content.nodes {
        match node {
            CallNode::Call(call) => output.extend(reduce(call, registry)?),
            CallNode::Element(element) => output.elements.push(element.clone()),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use notist_model::{Content, Element, ElementNode, TextRange};

    use super::*;
    use crate::{Function, FunctionOutput, FunctionRegistry, FunctionSignature, Parameter, Type};

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
