use crate::TextRange;

/// Whether a Content literal was written inline (`[...]` in Code) or as a
/// block payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyForm {
    Inline,
    Block,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpannedName {
    pub value: String,
    pub range: TextRange,
}

pub(crate) fn parse_qualified_name(source: &str, start: usize) -> Option<(SpannedName, usize)> {
    let (_, mut end) = parse_identifier(source, start)?;
    loop {
        if source.as_bytes().get(end..end + 2) != Some(b"::") {
            break;
        }
        let (_, segment_end) = parse_identifier(source, end + 2)?;
        end = segment_end;
    }

    Some((
        SpannedName {
            value: source[start..end].to_owned(),
            range: TextRange::new(start, end),
        },
        end,
    ))
}

pub(crate) fn parse_identifier(source: &str, start: usize) -> Option<(String, usize)> {
    let tail = source.get(start..)?;
    let mut end = start;
    for (offset, character) in tail.char_indices() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    (end > start).then(|| (source[start..end].to_owned(), end))
}
