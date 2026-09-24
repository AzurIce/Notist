use crate::{CoreError, TextEdit};

/// Validates a UTF-16 boundary and returns the corresponding UTF-8 byte index.
pub fn utf16_to_byte(text: &str, offset: usize) -> Result<usize, CoreError> {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units == offset {
            return Ok(byte);
        }
        units += c.len_utf16();
        if units > offset {
            return Err(CoreError::InvalidPosition { offset });
        }
    }
    if units == offset {
        Ok(text.len())
    } else {
        Err(CoreError::InvalidPosition { offset })
    }
}

/// A display delta, never fed back into the CRDT during an import/undo. Working
/// on scalar boundaries makes it safe for both UTF-8 and UTF-16 consumers.
pub(crate) fn difference(before: &str, after: &str) -> Vec<TextEdit> {
    if before == after {
        return vec![];
    }
    let prefix: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let suffix: usize = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    vec![TextEdit {
        from: before[..prefix].encode_utf16().count(),
        to: before[..before.len() - suffix].encode_utf16().count(),
        insert: after[prefix..after.len() - suffix].to_owned(),
    }]
}

pub(crate) fn validate_edits(text: &str, edits: &[TextEdit]) -> Result<Vec<TextEdit>, CoreError> {
    let mut previous: Option<&TextEdit> = None;
    let mut useful = Vec::new();
    for edit in edits {
        if edit.from > edit.to || previous.is_some_and(|p| p.to > edit.from || p.from == edit.from)
        {
            return Err(CoreError::InvalidEdits);
        }
        let from = utf16_to_byte(text, edit.from)?;
        let to = utf16_to_byte(text, edit.to)?;
        if text[from..to] != edit.insert {
            useful.push(edit.clone());
        }
        previous = Some(edit);
    }
    Ok(useful)
}
