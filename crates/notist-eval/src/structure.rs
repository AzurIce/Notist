use notist_model::{Block, Content, Element, StructuredDocument};

use crate::{Evaluation, StructuredEvaluation};

/// Groups an evaluated content sequence into paragraphs, lists, and blocks.
pub fn structure(evaluation: Evaluation) -> StructuredEvaluation {
    let mut blocks = Vec::new();
    let mut paragraph = Content::new();

    for node in evaluation.content.elements {
        if node.element.is_inline() {
            paragraph.elements.push(node);
            continue;
        }

        match node.element {
            Element::Parbreak => flush_paragraph(&mut paragraph, &mut blocks),
            Element::ListItem(_) => {
                flush_paragraph(&mut paragraph, &mut blocks);
                match blocks.last_mut() {
                    Some(Block::List(items)) => items.push(node),
                    _ => blocks.push(Block::List(vec![node])),
                }
            }
            _ => {
                flush_paragraph(&mut paragraph, &mut blocks);
                blocks.push(Block::Element(node));
            }
        }
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    StructuredEvaluation {
        document: StructuredDocument {
            blocks,
            annotations: evaluation.annotations,
        },
        diagnostics: evaluation.diagnostics,
    }
}

fn flush_paragraph(paragraph: &mut Content, blocks: &mut Vec<Block>) {
    if !paragraph.is_empty() {
        blocks.push(Block::Paragraph(std::mem::take(paragraph)));
    }
}
