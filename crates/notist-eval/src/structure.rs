use notist_model::{Block, Content, Element, ElementNode, StructuredDocument, TextRange};

use crate::{Evaluation, StructuredEvaluation};

/// Groups an evaluated content sequence into paragraphs, lists, tables, and blocks.
pub fn structure(evaluation: Evaluation) -> StructuredEvaluation {
    let mut blocks = Vec::new();
    let mut paragraph = Content::new();

    for node in evaluation.content.elements {
        if paragraph.is_empty()
            && matches!(&node.element, Element::Text(text) if text.trim().is_empty())
        {
            continue;
        }
        if node.element.is_inline() {
            paragraph.elements.push(node);
            continue;
        }

        match node.element {
            Element::Parbreak => flush_paragraph(&mut paragraph, &mut blocks),
            Element::ListItem(_) => {
                flush_paragraph(&mut paragraph, &mut blocks);
                push_list_item(&mut blocks, node, false);
            }
            Element::EnumItem { .. } => {
                flush_paragraph(&mut paragraph, &mut blocks);
                push_list_item(&mut blocks, node, true);
            }
            _ => {
                flush_paragraph(&mut paragraph, &mut blocks);
                blocks.push(Block::Element(node));
            }
        }
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    let blocks = group_sections(blocks);
    StructuredEvaluation {
        document: StructuredDocument { blocks },
        diagnostics: evaluation.diagnostics,
        annotations: evaluation.annotations,
    }
}

/// A heading plus its content while section grouping is in progress.
struct SectionBuilder {
    level: u8,
    heading: ElementNode,
    body: Vec<Block>,
}

/// D0002 shaping: each heading and its following content up to the next
/// same-or-higher-level heading form a Section node, recursively.
fn group_sections(blocks: Vec<Block>) -> Vec<Block> {
    let mut output = Vec::new();
    let mut open: Vec<SectionBuilder> = Vec::new();

    for block in blocks {
        let heading_level = match &block {
            Block::Element(ElementNode {
                element: Element::Heading { level, .. },
                ..
            }) => Some(*level),
            _ => None,
        };
        if let Some(level) = heading_level {
            while open.last().is_some_and(|section| section.level >= level) {
                let section = open.pop().unwrap();
                push_section(&mut output, &mut open, section);
            }
            let Block::Element(heading) = block else {
                unreachable!()
            };
            open.push(SectionBuilder {
                level,
                heading,
                body: Vec::new(),
            });
        } else if let Some(top) = open.last_mut() {
            top.body.push(block);
        } else {
            output.push(block);
        }
    }
    while let Some(section) = open.pop() {
        push_section(&mut output, &mut open, section);
    }
    output
}

fn push_section(output: &mut Vec<Block>, open: &mut [SectionBuilder], section: SectionBuilder) {
    let block = Block::Section {
        level: section.level,
        heading: section.heading,
        body: section.body,
    };
    match open.last_mut() {
        Some(parent) => parent.body.push(block),
        None => output.push(block),
    }
}

fn push_list_item(blocks: &mut Vec<Block>, node: ElementNode, ordered: bool) {
    match blocks.last_mut() {
        Some(Block::Element(ElementNode {
            element:
                Element::List {
                    ordered: existing,
                    items,
                },
            range,
        })) if *existing == ordered => {
            range.end = node.range.end;
            items.push(node);
        }
        _ => blocks.push(Block::Element(ElementNode {
            range: node.range,
            element: Element::List {
                ordered,
                items: vec![node],
            },
        })),
    }
}

fn flush_paragraph(paragraph: &mut Content, blocks: &mut Vec<Block>) {
    if !paragraph.is_empty() {
        let range = TextRange::new(
            paragraph.elements.first().unwrap().range.start,
            paragraph.elements.last().unwrap().range.end,
        );
        blocks.push(Block::Element(ElementNode {
            element: Element::Paragraph(std::mem::take(paragraph)),
            range,
        }));
    }
}
