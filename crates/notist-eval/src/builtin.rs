#![allow(dead_code)]

use notist_model::{Content, Element};

use crate::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
    FunctionSignature, RegistryError,
};

pub(crate) fn register_builtins(registry: &mut FunctionRegistry) -> Result<(), RegistryError> {
    registry.register(RefFunction)?;
    registry.register(HeadingFunction)?;
    registry.register(RawFunction)?;
    registry.register(CalloutFunction)?;
    registry.register(DetailsFunction)?;
    registry.register(ItemFunction)?;
    registry.register(StrongFunction)?;
    registry.register(EmphFunction)?;
    registry.register(StrikeFunction)?;
    registry.register(UnderlineFunction)?;
    registry.register(RuleFunction)?;
    Ok(())
}



struct RefFunction;

impl Function for RefFunction {
    fn name(&self) -> &str {
        "ref"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::ref_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let target = input.arguments.string("url");
        let reference = notist_syntax::parse_wiki_reference(target).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        Ok(FunctionOutput::content(Content::single(
            Element::Reference(reference),
            input.range,
        )))
    }
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
        if level < 1 {
            return Err(vec![EvalDiagnostic {
                message: "heading level must be at least 1".into(),
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
        let source = input.arguments.string("source").to_owned();
        let block = input.arguments.bool("block");
        if !block && source.contains('\n') {
            return Err(vec![EvalDiagnostic {
                message: "inline raw source must not contain line breaks".into(),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(raw_content(
            source,
            block,
            language,
            input.range,
        )))
    }
}



struct CalloutFunction;

impl Function for CalloutFunction {
    fn name(&self) -> &str {
        "callout"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::callout_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let kind = input.arguments.string("kind").trim().to_owned();
        if kind.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "callout kind cannot be empty".into(),
                range: input.range,
            }]);
        }
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Callout {
                kind,
                title: input.arguments.take_optional_content("title"),
                body,
            },
            input.range,
        )))
    }
}

struct DetailsFunction;

impl Function for DetailsFunction {
    fn name(&self) -> &str {
        "details"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::details_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let summary = input.arguments.take_optional_content("summary");
        let open = input.arguments.bool("open");
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Details {
                summary,
                open,
                body,
            },
            input.range,
        )))
    }
}

struct ItemFunction;

impl Function for ItemFunction {
    fn name(&self) -> &str {
        "item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let ordered = input.arguments.bool("ordered");
        let body = input.arguments.take_content("body");
        let element = if ordered {
            Element::EnumItem { value: None, body }
        } else {
            Element::ListItem(body)
        };
        Ok(FunctionOutput::content(Content::single(
            element,
            input.range,
        )))
    }
}

struct StrongFunction;

impl Function for StrongFunction {
    fn name(&self) -> &str {
        "strong"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Strong(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

struct EmphFunction;

impl Function for EmphFunction {
    fn name(&self) -> &str {
        "emph"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Emph(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

struct StrikeFunction;

impl Function for StrikeFunction {
    fn name(&self) -> &str {
        "strike"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Strike(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

macro_rules! inline_wrapper_function {
    ($function:ident, $name:literal, $variant:ident) => {
        struct $function;

        impl Function for $function {
            fn name(&self) -> &str {
                $name
            }

            fn signature(&self) -> FunctionSignature {
                notist_model::inline_body_signature()
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                mut input: FunctionInput<'_>,
            ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
                Ok(FunctionOutput::content(Content::single(
                    Element::$variant(input.arguments.take_content("body")),
                    input.range,
                )))
            }
        }
    };
}

inline_wrapper_function!(UnderlineFunction, "underline", Underline);







fn image_dimension(
    value: Option<i64>,
    name: &str,
    range: notist_model::TextRange,
) -> Result<Option<u32>, Vec<EvalDiagnostic>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !(1..=u32::MAX as i64).contains(&value) {
        return Err(vec![EvalDiagnostic {
            message: format!("image {name} must be between 1 and {}", u32::MAX),
            range,
        }]);
    }
    Ok(Some(value as u32))
}



struct RuleFunction;

impl Function for RuleFunction {
    fn name(&self) -> &str {
        "rule"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Rule,
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
    fn excludes_plugin_candidates_from_core_registry() {
        let evaluator = Evaluator::default();
        for name in [
            "outline",
            "terms",
            "insert",
            "spoiler",
            "highlight",
            "samp",
            "super",
            "sub",
            "footnote",
            "comment",
            "abbr",
            "time",
            "cite",
            "video",
            "audio",
        ] {
            let evaluation = evaluator.evaluate(&format!("#{name}[]"));
            assert!(
                evaluation
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message == format!("unknown function `{name}`")),
                "{name}: {:?}",
                evaluation.diagnostics
            );
        }
    }



    #[test]
    fn evaluates_ref_function_with_wiki_reference_validation() {
        let evaluator = Evaluator::default();
        let explicit = evaluator.evaluate("#ref(\"vault::guide::intro#overview\")");
        let sugar = evaluator.evaluate("[[vault::guide::intro#overview]]");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
        assert!(sugar.diagnostics.is_empty(), "{:?}", sugar.diagnostics);
        assert_eq!(
            explicit.content.elements[0].element,
            sugar.content.elements[0].element
        );

        let invalid = evaluator.evaluate("#ref(\"vault::::guide\")");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("empty segment") })
        );
    }

    #[test]
    fn evaluates_heading_and_raw() {
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
    }

    #[test]
    fn evaluates_ordered_and_unordered_items() {
        let evaluator = Evaluator::default();
        let unordered = evaluator.evaluate("#item[One]");
        assert!(
            unordered.diagnostics.is_empty(),
            "{:?}",
            unordered.diagnostics
        );
        assert!(matches!(
            unordered.content.elements[0].element,
            Element::ListItem(_)
        ));

        let ordered = evaluator.evaluate("#item(ordered=true)[First]");
        assert!(ordered.diagnostics.is_empty(), "{:?}", ordered.diagnostics);
        assert!(matches!(
            ordered.content.elements[0].element,
            Element::EnumItem { value: None, .. }
        ));
    }

    #[test]
    fn evaluates_callouts() {
        let evaluator = Evaluator::default();
        let default = evaluator.evaluate("#callout[Remember this]");
        assert!(default.diagnostics.is_empty(), "{:?}", default.diagnostics);
        assert!(matches!(
            &default.content.elements[0].element,
            Element::Callout { kind, title: None, body }
                if kind == "note" && matches!(&body.elements[0].element, Element::Text(text) if text == "Remember this")
        ));

        let titled = evaluator.evaluate("#callout(kind=\"warning\", title=[Risk])[Body]");
        assert!(titled.diagnostics.is_empty(), "{:?}", titled.diagnostics);
        assert!(matches!(
            &titled.content.elements[0].element,
            Element::Callout { title: Some(title), .. }
                if matches!(&title.elements[0].element, Element::Text(text) if text == "Risk")
        ));

        let empty = evaluator.evaluate("#callout(kind=\"\")[Body]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("kind cannot be empty"))
        );
    }

    #[test]
    fn evaluates_details() {
        let evaluated =
            Evaluator::default().evaluate("#details(summary=[More], open=true)[Hidden *content*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Details { summary: Some(summary), open: true, body }
                if matches!(&summary.elements[0].element, Element::Text(text) if text == "More")
                    && body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let without_summary = Evaluator::default().evaluate("#details[Hidden]");
        assert!(
            without_summary.diagnostics.is_empty(),
            "{:?}",
            without_summary.diagnostics
        );
        assert!(matches!(
            &without_summary.content.elements[0].element,
            Element::Details {
                summary: None,
                open: false,
                ..
            }
        ));
    }








    #[test]
    fn evaluates_strike_function() {
        let evaluated = Evaluator::default().evaluate("#strike[obsolete]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Strike(body)
                if matches!(&body.elements[0].element, Element::Text(text) if text == "obsolete")
        ));
    }






    #[test]
    fn evaluates_rule_function() {
        let evaluated = Evaluator::default().evaluate("#rule()");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Rule
        ));
    }

    #[test]
    fn block_raw_excludes_delimiter_line_breaks() {
        let evaluated = Evaluator::default().evaluate(
            "#raw(r#\"\"\"\nline one\nline two\n\"\"\"#, lang=\"text\", block=true)",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Raw { text, block: true, .. } if text == "line one\nline two"
        ));

        // D0003 constructor validation: an inline raw source must not contain
        // line breaks; block: true is the opt-in for multi-line sources.
        let invalid = Evaluator::default()
            .evaluate("#raw(\"line one\\nline two\")");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("must not contain line breaks") }),
            "{:?} {:#?}",
            invalid.diagnostics,
            invalid.content.elements
        );
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

        let unknown = evaluator.evaluate("#details(source=\"book\")[text]");
        assert_eq!(unknown.diagnostics[0].message, "unknown argument `source`");
    }






}
