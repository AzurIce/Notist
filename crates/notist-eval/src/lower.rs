use notist_model::{Content, Element, ElementNode, TextRange};
use notist_syntax::{
    BodyForm, Call, ContentBlock, EmbeddedExpression, Expression, ExpressionKind, Markup,
    MarkupItem, RawLiteral, RawLiteralForm,
};

use crate::function::{FunctionContext, FunctionInput, FunctionRegistry};
use crate::type_system::{Value, ValueOrigin, bind_arguments, evaluate_literal};
use crate::{EvalDiagnostic, Evaluation};

pub(crate) fn evaluate_markup(
    source: &str,
    markup: &Markup,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    let mut state = LowerState {
        source,
        base_offset,
        registry,
        depth,
        content: Content::default(),
        diagnostics: Vec::new(),
    };
    state.lower_markup(markup);
    Evaluation {
        content: state.content,
        diagnostics: state.diagnostics,
    }
}

struct LowerState<'a> {
    source: &'a str,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    depth: usize,
    content: Content,
    diagnostics: Vec<EvalDiagnostic>,
}

impl LowerState<'_> {
    fn lower_markup(&mut self, markup: &Markup) {
        for item in &markup.items {
            match item {
                MarkupItem::Text(text) => {
                    self.push_text_with_parbreaks(text);
                }
                MarkupItem::Wiki(link) => {
                    self.push_element(
                        Element::Reference(link.target.clone()),
                        link.range.shifted(self.base_offset),
                    );
                }
                MarkupItem::Raw(raw) => self.lower_raw(raw),
                MarkupItem::Embedded(embedded) => self.lower_embedded(embedded),
            }
        }
    }

    fn lower_embedded(&mut self, embedded: &EmbeddedExpression) {
        let (value, _, mut diagnostics) =
            self.evaluate_expression(&embedded.expression, embedded.scope_range);
        self.diagnostics.append(&mut diagnostics);
        match value {
            Value::Content(content) => {
                self.content.elements.extend(content.elements);
            }
            Value::String(text) => {
                self.push_element(
                    Element::Text(text),
                    embedded.scope_range.shifted(self.base_offset),
                );
            }
            Value::None => {}
            other => {
                self.diagnostics.push(EvalDiagnostic {
                    message: format!("cannot insert {} into Markup", other.ty()),
                    range: embedded.expression.range.shifted(self.base_offset),
                });
            }
        }
    }

    fn lower_raw(&mut self, raw: &RawLiteral) {
        let payload_range = raw.payload_range.shifted(self.base_offset);
        let payload = &self.source[payload_range.start..payload_range.end];
        let block = raw.form == RawLiteralForm::Fenced;
        let language = raw
            .tag
            .as_ref()
            .map(|tag| tag.value.clone())
            .filter(|v| !v.is_empty());
        self.push_element(
            Element::Raw {
                text: payload.to_owned(),
                block,
                language,
            },
            raw.range.shifted(self.base_offset),
        );
    }

    fn evaluate_expression(
        &mut self,
        expression: &Expression,
        expression_range: TextRange,
    ) -> (Value, ValueOrigin, Vec<EvalDiagnostic>) {
        if let Some((value, origin)) = evaluate_literal(expression, self.base_offset) {
            return (value, origin, Vec::new());
        }
        match &expression.kind {
            ExpressionKind::Content(block) => {
                let (value, diagnostics) = self.evaluate_content_block(block);
                (
                    value,
                    ValueOrigin::ContentLiteral {
                        range: block.range.shifted(self.base_offset),
                    },
                    diagnostics,
                )
            }
            ExpressionKind::Call(call) => {
                let (value, diagnostics) = self.evaluate_call(call, expression_range);
                (value, ValueOrigin::Default, diagnostics)
            }
            ExpressionKind::Parenthesized(inner) => self.evaluate_expression(inner, inner.range),
            ExpressionKind::Error => (Value::None, ValueOrigin::Default, Vec::new()),
            _ => (
                Value::None,
                ValueOrigin::Default,
                vec![EvalDiagnostic {
                    message: "unsupported expression in evaluation".into(),
                    range: expression_range.shifted(self.base_offset),
                }],
            ),
        }
    }

    fn evaluate_content_block(&mut self, block: &ContentBlock) -> (Value, Vec<EvalDiagnostic>) {
        let evaluation = evaluate_markup(
            self.source,
            &block.markup,
            self.base_offset,
            self.registry,
            self.depth,
        );
        let diagnostics = evaluation.diagnostics;
        (Value::Content(evaluation.content), diagnostics)
    }

    fn evaluate_call(
        &mut self,
        call: &Call,
        site_range: TextRange,
    ) -> (Value, Vec<EvalDiagnostic>) {
        let mut diagnostics = Vec::new();
        let name = &call.name.value;

        let Some(function) = self.registry.get(name) else {
            self.diagnostics.push(EvalDiagnostic {
                message: format!("unknown function `{name}`"),
                range: call.name.range.shifted(self.base_offset),
            });
            let (trailing, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
            diagnostics.append(&mut trailing_diagnostics);
            let arguments = call.arguments_range.map(|range| {
                let absolute = range.shifted(self.base_offset);
                self.source[absolute.start..absolute.end].to_owned()
            });
            let block = call
                .trailing
                .iter()
                .any(|block| block.form == BodyForm::Block);
            let content = Content::single(
                Element::UnresolvedCall {
                    name: name.clone(),
                    arguments,
                    trailing: trailing.into_iter().map(|(content, _)| content).next(),
                    block,
                },
                site_range.shifted(self.base_offset),
            );
            return (Value::Content(content), diagnostics);
        };

        let signature = function.signature();
        let (trailing_content, mut trailing_diagnostics) = self.evaluate_trailing(&call.trailing);
        diagnostics.append(&mut trailing_diagnostics);

        let bound = match bind_arguments(
            &signature,
            &call.arguments,
            trailing_content,
            call.name.range,
            self.base_offset,
            |expression| {
                let (value, origin, diagnostics) =
                    self.evaluate_expression(expression, expression.range);
                if diagnostics.is_empty() {
                    Ok((value, origin))
                } else {
                    Err(diagnostics)
                }
            },
        ) {
            Ok(bound) => bound,
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                return (Value::None, diagnostics);
            }
        };

        let context = FunctionContext {
            registry: self.registry,
            depth: self.depth,
        };
        let input = FunctionInput {
            name,
            arguments: bound,
            range: site_range.shifted(self.base_offset),
        };
        match function.call(&context, input) {
            Ok(output) => (Value::Content(output.content), diagnostics),
            Err(mut errors) => {
                diagnostics.append(&mut errors);
                (Value::None, diagnostics)
            }
        }
    }

    fn evaluate_trailing(
        &mut self,
        trailing: &[ContentBlock],
    ) -> (Vec<(Content, TextRange)>, Vec<EvalDiagnostic>) {
        let mut result = Vec::new();
        let mut diagnostics = Vec::new();
        for block in trailing {
            let evaluation = evaluate_markup(
                self.source,
                &block.markup,
                self.base_offset,
                self.registry,
                self.depth + 1,
            );
            diagnostics.extend(evaluation.diagnostics);
            let range = block.payload_range.shifted(self.base_offset);
            result.push((evaluation.content, range));
        }
        (result, diagnostics)
    }

    fn push_element(&mut self, element: Element, range: TextRange) {
        self.content.elements.push(ElementNode { element, range });
    }

    fn push_text_with_parbreaks(&mut self, text: &notist_syntax::SpannedText) {
        let bytes = text.value.as_bytes();
        let mut segment_start = 0usize;
        let mut cursor = 0usize;

        while cursor < bytes.len() {
            let is_line_break = matches!(bytes.get(cursor), Some(b'\n' | b'\r'));
            if !is_line_break {
                cursor += 1;
                continue;
            }
            let line_end = if bytes.get(cursor..cursor + 2) == Some(b"\r\n") {
                cursor + 2
            } else {
                cursor + 1
            };
            let mut after = line_end;
            while after < bytes.len() && bytes[after] == b'\n' {
                after += 1;
            }
            if after == line_end {
                cursor = line_end;
                continue;
            }
            let text_end = cursor;
            if segment_start < text_end {
                self.push_element(
                    Element::Text(
                        String::from_utf8_lossy(&bytes[segment_start..text_end]).into_owned(),
                    ),
                    TextRange::new(
                        text.range.start + segment_start,
                        text.range.start + text_end,
                    ),
                );
            }
            self.push_element(
                Element::Parbreak,
                TextRange::new(text.range.start + text_end, text.range.start + after),
            );
            segment_start = after;
            cursor = after;
        }

        if segment_start < bytes.len() {
            self.push_element(
                Element::Text(String::from_utf8_lossy(&bytes[segment_start..]).into_owned()),
                TextRange::new(text.range.start + segment_start, text.range.end),
            );
        }
    }
}
