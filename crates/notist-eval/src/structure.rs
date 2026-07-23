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
            Element::TermItem { .. } => {
                flush_paragraph(&mut paragraph, &mut blocks);
                push_terms_item(&mut blocks, node);
            }
            Element::TaskItem { .. } => {
                flush_paragraph(&mut paragraph, &mut blocks);
                push_task_item(&mut blocks, node);
            }
            _ => {
                flush_paragraph(&mut paragraph, &mut blocks);
                blocks.push(Block::Element(node));
            }
        }
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    StructuredEvaluation {
        document: StructuredDocument { blocks },
        diagnostics: evaluation.diagnostics,
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

fn push_terms_item(blocks: &mut Vec<Block>, node: ElementNode) {
    match blocks.last_mut() {
        Some(Block::Element(ElementNode {
            element: Element::Terms { items },
            range,
        })) => {
            range.end = node.range.end;
            items.push(node);
        }
        _ => blocks.push(Block::Element(ElementNode {
            range: node.range,
            element: Element::Terms { items: vec![node] },
        })),
    }
}

fn push_task_item(blocks: &mut Vec<Block>, node: ElementNode) {
    match blocks.last_mut() {
        Some(Block::Element(ElementNode {
            element: Element::Tasks { items },
            range,
        })) => {
            range.end = node.range.end;
            items.push(node);
        }
        _ => blocks.push(Block::Element(ElementNode {
            range: node.range,
            element: Element::Tasks { items: vec![node] },
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
