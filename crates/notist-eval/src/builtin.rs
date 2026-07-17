use notist_model::{Content, Element};
use notist_syntax::StringLiteralForm;

use crate::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
    FunctionSignature, RegistryError,
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
        notist_model::heading_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let level = input.arguments.int("level");
        if !(1..=6).contains(&level) {
            return Err(vec![EvalDiagnostic {
                message: "heading level must be between 1 and 6".into(),
                range: input.range,
            }]);
        }
        let body = input.arguments.take_content("body");
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
        notist_model::raw_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let language = input.arguments.optional_string("lang").map(str::to_owned);
        let text = input.arguments.string("text").to_owned();
        let block = input.arguments.string_form("text") == Some(StringLiteralForm::Multiline);
        Ok(FunctionOutput::content(raw_content(
            text,
            block,
            language,
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
        notist_model::quote_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Quote(body),
            input.range,
        )))
    }
}

pub(crate) fn raw_content(
    text: String,
    block: bool,
    language: Option<String>,
    range: notist_model::TextRange,
) -> Content {
    Content::single(
        Element::Raw {
            text,
            block,
            language,
        },
        range,
    )
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

        let raw = evaluator.evaluate("#raw(r#\"fn main() {}\"#, lang=\"rust\")");
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
    fn block_raw_excludes_delimiter_line_breaks() {
        let evaluated = Evaluator::default()
            .evaluate("#raw(r#\"\"\"\nline one\nline two\n\"\"\"#, lang=\"text\")");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Raw { text, block: true, .. } if text == "line one\nline two"
        ));
    }

    #[test]
    fn raw_triple_quotes_without_an_opening_line_break_stay_inline() {
        let evaluated = Evaluator::default().evaluate(r####"#raw(r#"""quoted"""#)"####);
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Raw { text, block: false, .. } if text == "\"\"quoted\"\""
        ));
    }

    #[test]
    fn reports_signature_and_trailing_content_errors_before_calling_builtins() {
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
            "function does not accept trailing Content"
        );

        let unknown = evaluator.evaluate("#quote(source=\"book\")[text]");
        assert_eq!(unknown.diagnostics[0].message, "unknown argument `source`");
    }
}
