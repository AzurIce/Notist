use notist_model::{Content, Element};
use notist_syntax::BodyForm;

use crate::{
    CallBody, DefaultValue, EvalDiagnostic, Function, FunctionContext, FunctionInput,
    FunctionOutput, FunctionRegistry, FunctionSignature, Parameter, RegistryError, Type,
};

pub(crate) fn register_builtins(registry: &mut FunctionRegistry) -> Result<(), RegistryError> {
    registry.register(HeadingFunction)?;
    registry.register(RawFunction)?;
    registry.register(QuoteFunction)?;
    Ok(())
}

struct HeadingFunction;

impl Function for HeadingFunction {
    fn name(&self) -> &str {
        "heading"
    }

    fn signature(&self) -> FunctionSignature {
        FunctionSignature {
            parameters: vec![Parameter {
                name: "level",
                ty: Type::Int,
                default: Some(DefaultValue::Int(1)),
            }],
            body: Type::Content,
            result: Type::Content,
        }
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let level = input.arguments.int("level");
        if !(1..=6).contains(&level) {
            return Err(vec![EvalDiagnostic {
                message: "heading level must be between 1 and 6".into(),
                range: input.range,
            }]);
        }
        let CallBody::Content(body) = input.body else {
            unreachable!("signature binding guarantees a Content body");
        };
        Ok(FunctionOutput::content(Content::single(
            Element::Heading {
                level: level as u8,
                body,
            },
            input.range,
        )))
    }
}

struct RawFunction;

impl Function for RawFunction {
    fn name(&self) -> &str {
        "raw"
    }

    fn signature(&self) -> FunctionSignature {
        FunctionSignature {
            parameters: vec![Parameter {
                name: "lang",
                ty: Type::Optional(Box::new(Type::String)),
                default: Some(DefaultValue::None),
            }],
            body: Type::RawSource,
            result: Type::Content,
        }
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let language = input.arguments.optional_string("lang").map(str::to_owned);
        let CallBody::Raw(body) = input.body else {
            unreachable!("signature binding guarantees a RawSource body");
        };
        Ok(FunctionOutput::content(Content::single(
            Element::Raw {
                text: body.text.to_owned(),
                block: input.body_form == BodyForm::Block,
                language,
            },
            input.range,
        )))
    }
}

struct QuoteFunction;

impl Function for QuoteFunction {
    fn name(&self) -> &str {
        "quote"
    }

    fn signature(&self) -> FunctionSignature {
        FunctionSignature {
            parameters: Vec::new(),
            body: Type::Content,
            result: Type::Content,
        }
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let CallBody::Content(body) = input.body else {
            unreachable!("signature binding guarantees a Content body");
        };
        Ok(FunctionOutput::content(Content::single(
            Element::Quote(body),
            input.range,
        )))
    }
}

#[cfg(test)]
mod tests {
    use notist_model::Element;

    use crate::Evaluator;

    #[test]
    fn evaluates_heading_raw_and_quote() {
        let evaluator = Evaluator::default();

        let heading = evaluator.evaluate("#heading(level=2)[Title]");
        assert!(heading.diagnostics.is_empty(), "{:?}", heading.diagnostics);
        assert!(matches!(
            &heading.content.elements[0].element,
            Element::Heading { level: 2, .. }
        ));

        let raw = evaluator.evaluate("#raw(lang=\"rust\")![fn main() {}]!");
        assert!(raw.diagnostics.is_empty(), "{:?}", raw.diagnostics);
        assert!(matches!(
            &raw.content.elements[0].element,
            Element::Raw { text, language, block: false }
                if text == "fn main() {}" && language.as_deref() == Some("rust")
        ));

        let quote = evaluator.evaluate("#quote[Quoted [[self::target]]]");
        assert!(quote.diagnostics.is_empty(), "{:?}", quote.diagnostics);
        assert!(matches!(
            &quote.content.elements[0].element,
            Element::Quote(body) if matches!(body.elements[1].element, Element::Reference(_))
        ));
    }

    #[test]
    fn reports_signature_and_body_mode_errors_before_calling_builtins() {
        let evaluator = Evaluator::default();
        let wrong_level = evaluator.evaluate("#heading(level=\"two\")[Title]");
        assert!(
            wrong_level.diagnostics[0]
                .message
                .contains("expected Int, found String")
        );

        let wrong_body = evaluator.evaluate("#raw[parsed]");
        assert_eq!(
            wrong_body.diagnostics[0].message,
            "body type mismatch: expected RawSource, found Content"
        );

        let unknown = evaluator.evaluate("#quote(source=\"book\")[text]");
        assert_eq!(unknown.diagnostics[0].message, "unknown argument `source`");
    }
}
