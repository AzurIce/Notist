use notist_model::{Annotation, Content, Element, ElementNode, Metadata, Property, TextRange};
use notist_syntax::{Attribute, Attributes, OpaqueScope, Parse, Scope, TransparentScope, WikiLink};

use crate::processor::{ProcessContext, ProcessorInput, ProcessorRegistry, RawSource};
use crate::{EvalDiagnostic, Evaluation};

pub(crate) fn lower_parsed(
    source: &str,
    parse: &Parse,
    base_offset: usize,
    registry: &ProcessorRegistry,
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
    registry: &'a ProcessorRegistry,
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
                Event::Scope(Scope::Transparent(scope)) => {
                    self.lower_transparent(&scope, &mut content);
                    cursor = scope.range.end;
                }
                Event::Scope(Scope::Opaque(scope)) => {
                    self.lower_opaque(&scope, &mut content);
                    cursor = scope.range.end;
                }
                Event::Link(link) => {
                    content.elements.push(ElementNode {
                        element: Element::Reference(link.target),
                        range: link.range.shifted(self.base_offset),
                    });
                    cursor = link.range.end;
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

    fn lower_opaque(&mut self, scope: &OpaqueScope, content: &mut Content) {
        let metadata = lower_metadata(&scope.attributes);
        if !metadata.is_empty() {
            self.annotations.push(Annotation {
                range: scope.body_range.shifted(self.base_offset),
                metadata,
            });
        }

        let body_text = &self.source[scope.body_range.start..scope.body_range.end];
        let arguments = scope
            .arguments_range
            .map(|range| &self.source[range.start..range.end]);
        let global_body_range = scope.body_range.shifted(self.base_offset);
        let global_scope_range = scope.range.shifted(self.base_offset);

        let Some(processor) = self.registry.get(&scope.name.value) else {
            self.diagnostics.push(EvalDiagnostic {
                message: format!("unknown processor `{}`", scope.name.value),
                range: scope.name.range.shifted(self.base_offset),
            });
            content.elements.push(ElementNode {
                element: Element::UnresolvedProcessor {
                    name: scope.name.value.clone(),
                    arguments: arguments.map(str::to_owned),
                    body: body_text.to_owned(),
                    block: body_text.contains('\n'),
                },
                range: global_scope_range,
            });
            return;
        };

        let context = ProcessContext {
            registry: self.registry,
            depth: self.depth,
        };
        let input = ProcessorInput {
            name: &scope.name.value,
            arguments,
            body: RawSource {
                text: body_text,
                range: global_body_range,
            },
            range: global_scope_range,
        };

        match processor.process(&context, input) {
            Ok(output) => {
                content.extend(output.content);
                self.annotations.extend(output.annotations);
            }
            Err(diagnostics) => self.diagnostics.extend(diagnostics),
        }
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
            .filter(|scope| scope.range().start >= cursor && scope.range().end <= end)
            .min_by_key(|scope| scope.range().start)
            .cloned()
            .map(Event::Scope);
        let link = self
            .parse
            .links
            .iter()
            .filter(|link| link.range.start >= cursor && link.range.end <= end)
            .min_by_key(|link| link.range.start)
            .cloned()
            .map(Event::Link);

        match (scope, link) {
            (Some(scope), Some(link)) => {
                if scope.start() <= link.start() {
                    Some(scope)
                } else {
                    Some(link)
                }
            }
            (Some(scope), None) => Some(scope),
            (None, Some(link)) => Some(link),
            (None, None) => None,
        }
    }
}

enum Event {
    Scope(Scope),
    Link(WikiLink),
}

impl Event {
    fn start(&self) -> usize {
        match self {
            Self::Scope(scope) => scope.range().start,
            Self::Link(link) => link.range.start,
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
