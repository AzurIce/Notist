use notist_model::{Annotation, Content, Element, ElementNode, Metadata, Property, TextRange};
use notist_syntax::{
    Attribute, Attributes, BodyForm, Call, Parse, RawLiteral, RawLiteralForm, TransparentScope,
    WikiLink,
};

use crate::builtin::raw_content;
use crate::function::{FunctionContext, FunctionInput, FunctionRegistry};
use crate::type_system::bind_arguments;
use crate::{EvalDiagnostic, Evaluation};

pub(crate) fn lower_parsed(
    source: &str,
    parse: &Parse,
    base_offset: usize,
    registry: &FunctionRegistry,
    depth: usize,
) -> Evaluation {
    let mut lowerer = Lowerer {
        source,
        parse,
        base_offset,
        registry,
        depth,
        annotations: Vec::new(),
        diagnostics: parse
            .errors
            .iter()
            .map(|error| EvalDiagnostic {
                message: error.message.clone(),
                range: error.range.shifted(base_offset),
            })
            .collect(),
    };
    let content = lowerer.lower_range(TextRange::new(0, source.len()));
    Evaluation {
        content,
        annotations: lowerer.annotations,
        diagnostics: lowerer.diagnostics,
    }
}

struct Lowerer<'a> {
    source: &'a str,
    parse: &'a Parse,
    base_offset: usize,
    registry: &'a FunctionRegistry,
    depth: usize,
    annotations: Vec<Annotation>,
    diagnostics: Vec<EvalDiagnostic>,
}

impl Lowerer<'_> {
    fn lower_range(&mut self, range: TextRange) -> Content {
        let mut content = Content::new();
        let mut cursor = range.start;

        while let Some(event) = self.next_event(cursor, range.end) {
            if cursor < event.start() {
                self.lower_text(TextRange::new(cursor, event.start()), &mut content);
            }

            match event {
                Event::Scope(scope) => {
                    self.lower_transparent(&scope, &mut content);
                    cursor = scope.range.end;
                }
                Event::Call(call) => {
                    self.lower_call(&call, &mut content);
                    cursor = call.range.end;
                }
                Event::Link(link) => {
                    content.elements.push(ElementNode {
                        element: Element::Reference(link.target),
                        range: link.range.shifted(self.base_offset),
                    });
                    cursor = link.range.end;
                }
                Event::Raw(raw) => {
                    self.lower_raw_literal(&raw, &mut content);
                    cursor = raw.range.end;
                }
            }
        }

        if cursor < range.end {
            self.lower_text(TextRange::new(cursor, range.end), &mut content);
        }
        content
    }

    fn lower_transparent(&mut self, scope: &TransparentScope, content: &mut Content) {
        let metadata = lower_metadata(&scope.attributes);
        if !metadata.is_empty() {
            self.annotations.push(Annotation {
                range: scope.body_range.shifted(self.base_offset),
                metadata,
            });
        }
        content.extend(self.lower_range(scope.body_range));
    }

    fn lower_call(&mut self, call: &Call, content: &mut Content) {
        let metadata = lower_metadata(&call.attributes);
        if !metadata.is_empty() {
            let annotation_range = call
                .body
                .as_ref()
                .map(|body| body.payload_range)
                .unwrap_or_else(|| {
                    TextRange::new(
                        call.range.start,
                        call.attributes
                            .range
                            .map_or(call.range.end, |attributes| attributes.start),
                    )
                });
            self.annotations.push(Annotation {
                range: annotation_range.shifted(self.base_offset),
                metadata,
            });
        }

        let global_call_range = call.range.shifted(self.base_offset);
        let trailing = call
            .body
            .as_ref()
            .map(|body| (self.lower_range(body.payload_range), body.payload_range));
        let Some(function) = self.registry.get(&call.name.value) else {
            self.diagnostics.push(EvalDiagnostic {
                message: format!("unknown function `{}`", call.name.value),
                range: call.name.range.shifted(self.base_offset),
            });
            content.elements.push(ElementNode {
                element: Element::UnresolvedCall {
                    name: call.name.value.clone(),
                    arguments: call
                        .arguments_range
                        .map(|range| self.source[range.start..range.end].to_owned()),
                    trailing: trailing.map(|(content, _)| content),
                    block: call
                        .body
                        .as_ref()
                        .is_some_and(|body| body.form == BodyForm::Block),
                },
                range: global_call_range,
            });
            return;
        };

        let arguments = match bind_arguments(
            &function.signature(),
            &call.arguments,
            trailing,
            call.name.range,
            self.base_offset,
        ) {
            Ok(arguments) => arguments,
            Err(diagnostics) => {
                self.diagnostics.extend(diagnostics);
                return;
            }
        };

        let context = FunctionContext {
            registry: self.registry,
            depth: self.depth,
        };
        let input = FunctionInput {
            name: &call.name.value,
            arguments,
            range: global_call_range,
        };

        match function.call(&context, input) {
            Ok(output) => {
                content.extend(output.content);
                self.annotations.extend(output.annotations);
            }
            Err(diagnostics) => self.diagnostics.extend(diagnostics),
        }
    }

    fn lower_raw_literal(&self, raw: &RawLiteral, content: &mut Content) {
        let text = self.source[raw.payload_range.start..raw.payload_range.end].to_owned();
        let language = raw.tag.as_ref().map(|tag| tag.value.clone());
        content.extend(raw_content(
            text,
            raw.form == RawLiteralForm::Fenced,
            language,
            raw.range.shifted(self.base_offset),
        ));
    }

    fn lower_text(&self, range: TextRange, content: &mut Content) {
        let bytes = self.source.as_bytes();
        let mut segment_start = range.start;
        let mut cursor = range.start;

        while cursor < range.end {
            if bytes[cursor] != b'\n' {
                cursor += 1;
                continue;
            }

            let mut lookahead = cursor + 1;
            while lookahead < range.end && matches!(bytes[lookahead], b' ' | b'\t' | b'\r') {
                lookahead += 1;
            }
            if lookahead >= range.end || bytes[lookahead] != b'\n' {
                cursor += 1;
                continue;
            }

            self.push_text(TextRange::new(segment_start, cursor), content);
            let break_end = lookahead + 1;
            content.elements.push(ElementNode {
                element: Element::Parbreak,
                range: TextRange::new(cursor + self.base_offset, break_end + self.base_offset),
            });
            cursor = break_end;
            segment_start = break_end;
        }

        self.push_text(TextRange::new(segment_start, range.end), content);
    }

    fn push_text(&self, range: TextRange, content: &mut Content) {
        if range.is_empty() {
            return;
        }
        let text = &self.source[range.start..range.end];
        if text.is_empty() {
            return;
        }
        content.elements.push(ElementNode {
            element: Element::Text(text.to_owned()),
            range: range.shifted(self.base_offset),
        });
    }

    fn next_event(&self, cursor: usize, end: usize) -> Option<Event> {
        let scope = self
            .parse
            .scopes
            .iter()
            .filter(|scope| scope.range.start >= cursor && scope.range.end <= end)
            .min_by_key(|scope| scope.range.start)
            .cloned()
            .map(Event::Scope);
        let call = self
            .parse
            .calls
            .iter()
            .filter(|call| call.range.start >= cursor && call.range.end <= end)
            .min_by_key(|call| call.range.start)
            .cloned()
            .map(Event::Call);
        let link = self
            .parse
            .links
            .iter()
            .filter(|link| link.range.start >= cursor && link.range.end <= end)
            .min_by_key(|link| link.range.start)
            .cloned()
            .map(Event::Link);
        let raw = self
            .parse
            .raw_literals
            .iter()
            .filter(|raw| raw.range.start >= cursor && raw.range.end <= end)
            .min_by_key(|raw| raw.range.start)
            .cloned()
            .map(Event::Raw);

        [scope, call, link, raw]
            .into_iter()
            .flatten()
            .min_by_key(Event::start)
    }
}

enum Event {
    Scope(TransparentScope),
    Call(Call),
    Link(WikiLink),
    Raw(RawLiteral),
}

impl Event {
    fn start(&self) -> usize {
        match self {
            Self::Scope(scope) => scope.range.start,
            Self::Call(call) => call.range.start,
            Self::Link(link) => link.range.start,
            Self::Raw(raw) => raw.range.start,
        }
    }
}

fn lower_metadata(attributes: &Attributes) -> Metadata {
    let mut metadata = Metadata {
        id: attributes.id.as_ref().map(|id| id.value.clone()),
        ..Metadata::default()
    };
    for attribute in &attributes.items {
        match attribute {
            Attribute::Tag(tag) => metadata.tags.push(tag.value.clone()),
            Attribute::Class(class) => metadata.classes.push(class.value.clone()),
            Attribute::KeyValue { key, value, .. } => metadata.properties.push(Property {
                key: key.value.clone(),
                value: value.raw.clone(),
            }),
        }
    }
    metadata
}
