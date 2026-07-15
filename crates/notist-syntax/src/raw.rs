use notist_model::TextRange;

use crate::SyntaxError;

pub(crate) struct RawParse {
    pub literals: Vec<RawLiteral>,
    pub errors: Vec<SyntaxError>,
}

/// A backtick-delimited raw source literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawLiteral {
    /// The source payload without the surrounding backtick delimiters.
    pub payload_range: TextRange,
    /// Whether the literal is an inline code span or a multiline fence.
    pub form: RawLiteralForm,
    /// The number of backticks in the opening delimiter.
    pub delimiter_len: usize,
    /// The optional info tag following a fenced opening delimiter.
    pub tag: Option<SpannedText>,
    /// The complete literal range including delimiters.
    pub range: TextRange,
}

/// The source form of a backtick raw literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawLiteralForm {
    /// A code span delimited on the same logical line as its opener.
    Inline,
    /// A fenced block whose opening delimiter is followed by a newline.
    Fenced,
}

/// Arbitrary source text together with its range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpannedText {
    /// The text exactly as written in source.
    pub value: String,
    /// The source range occupied by the text.
    pub range: TextRange,
}

pub(crate) fn parse_raw_literals(source: &str) -> RawParse {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut errors = Vec::new();
    let mut cursor = 0;
    let mut argument_depth = 0usize;

    while cursor < bytes.len() {
        if argument_depth > 0 {
            if matches!(bytes[cursor], b'"' | b'r')
                && let Some(string) = crate::argument::string_literal_range_at(source, cursor)
            {
                cursor = string.end;
                continue;
            }
            if bytes[cursor] == b'(' {
                argument_depth += 1;
                cursor += 1;
                continue;
            }
            if bytes[cursor] == b')' {
                argument_depth -= 1;
                cursor += 1;
                continue;
            }
        } else if bytes[cursor] == b'#'
            && let Some(open) = call_argument_open_at(source, cursor)
        {
            argument_depth = 1;
            cursor = open + 1;
            continue;
        }

        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }

        let (literal, error) = parse_at(source, cursor);
        cursor = literal.range.end.max(cursor + 1);
        errors.extend(error);
        literals.push(literal);
    }

    RawParse { literals, errors }
}

fn call_argument_open_at(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = start + 1;
    let mut segment_start = cursor;

    loop {
        while let Some(character) = source.get(cursor..)?.chars().next() {
            if character.is_alphanumeric() || matches!(character, '_' | '-') {
                cursor += character.len_utf8();
            } else {
                break;
            }
        }
        if cursor == segment_start {
            return None;
        }
        if bytes.get(cursor..cursor + 2) != Some(b"::") {
            break;
        }
        cursor += 2;
        segment_start = cursor;
    }

    (bytes.get(cursor) == Some(&b'(')).then_some(cursor)
}

pub(crate) fn parse_at(source: &str, start: usize) -> (RawLiteral, Option<SyntaxError>) {
    let bytes = source.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'`'));

    let opening_end = backtick_run_end(bytes, start);
    let delimiter_len = opening_end - start;
    let line_end = find_line_end(bytes, opening_end);
    let form = if delimiter_len >= 3
        && line_end.is_some()
        && find_inline_close(bytes, opening_end, line_end.unwrap(), delimiter_len).is_none()
    {
        RawLiteralForm::Fenced
    } else {
        RawLiteralForm::Inline
    };

    match form {
        RawLiteralForm::Inline => parse_inline(source, start, opening_end, delimiter_len),
        RawLiteralForm::Fenced => parse_fenced(source, start, opening_end, delimiter_len),
    }
}

fn parse_inline(
    source: &str,
    start: usize,
    opening_end: usize,
    delimiter_len: usize,
) -> (RawLiteral, Option<SyntaxError>) {
    let bytes = source.as_bytes();
    let mut cursor = opening_end;

    while cursor < bytes.len() {
        if matches!(bytes[cursor], b'\r' | b'\n') {
            return unclosed_inline(start, opening_end, cursor, delimiter_len);
        }
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let closing_end = backtick_run_end(bytes, cursor);
        if closing_end - cursor == delimiter_len {
            return (
                RawLiteral {
                    payload_range: TextRange::new(opening_end, cursor),
                    form: RawLiteralForm::Inline,
                    delimiter_len,
                    tag: None,
                    range: TextRange::new(start, closing_end),
                },
                None,
            );
        }
        cursor = closing_end;
    }

    unclosed_inline(start, opening_end, source.len(), delimiter_len)
}

fn unclosed_inline(
    start: usize,
    opening_end: usize,
    end: usize,
    delimiter_len: usize,
) -> (RawLiteral, Option<SyntaxError>) {
    let error = SyntaxError {
        message: format!(
            "unclosed inline raw literal; expected {} backtick{}",
            delimiter_len,
            if delimiter_len == 1 { "" } else { "s" }
        ),
        range: TextRange::new(start, end),
    };
    (
        RawLiteral {
            payload_range: TextRange::new(opening_end, end),
            form: RawLiteralForm::Inline,
            delimiter_len,
            tag: None,
            range: TextRange::new(start, end),
        },
        Some(error),
    )
}

fn parse_fenced(
    source: &str,
    start: usize,
    opening_end: usize,
    delimiter_len: usize,
) -> (RawLiteral, Option<SyntaxError>) {
    let bytes = source.as_bytes();
    let line_end = find_line_end(bytes, opening_end).expect("fenced opener has a newline");
    let tag_range = trim_horizontal(source, TextRange::new(opening_end, line_end));
    let tag = (!tag_range.is_empty()).then(|| SpannedText {
        value: source[tag_range.start..tag_range.end].to_owned(),
        range: tag_range,
    });
    let payload_start = newline_end(bytes, line_end);
    let mut cursor = payload_start;

    while cursor < bytes.len() {
        let line_start = cursor;
        let content_start = skip_horizontal(bytes, line_start);
        if bytes.get(content_start) == Some(&b'`') {
            let closing_end = backtick_run_end(bytes, content_start);
            if closing_end - content_start >= delimiter_len && valid_fence_tail(bytes, closing_end)
            {
                return (
                    RawLiteral {
                        payload_range: TextRange::new(
                            payload_start,
                            trim_framing_newline(bytes, line_start, payload_start),
                        ),
                        form: RawLiteralForm::Fenced,
                        delimiter_len,
                        tag,
                        range: TextRange::new(start, closing_end),
                    },
                    None,
                );
            }
        }
        cursor = match find_line_end(bytes, line_start) {
            Some(end) => newline_end(bytes, end),
            None => bytes.len(),
        };
    }

    let error = SyntaxError {
        message: format!(
            "unclosed fenced raw literal; expected a closing fence of at least {} backticks",
            delimiter_len
        ),
        range: TextRange::new(start, source.len()),
    };
    (
        RawLiteral {
            payload_range: TextRange::new(payload_start, source.len()),
            form: RawLiteralForm::Fenced,
            delimiter_len,
            tag,
            range: TextRange::new(start, source.len()),
        },
        Some(error),
    )
}

fn find_inline_close(
    bytes: &[u8],
    mut cursor: usize,
    end: usize,
    delimiter_len: usize,
) -> Option<usize> {
    while cursor < end {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let run_end = backtick_run_end(bytes, cursor);
        if run_end - cursor == delimiter_len {
            return Some(cursor);
        }
        cursor = run_end;
    }
    None
}

fn backtick_run_end(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes.get(cursor) == Some(&b'`') {
        cursor += 1;
    }
    cursor
}

fn find_line_end(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')
        .map(|relative| start + relative)
}

fn newline_end(bytes: &[u8], line_end: usize) -> usize {
    if bytes.get(line_end..line_end + 2) == Some(b"\r\n") {
        line_end + 2
    } else {
        line_end + 1
    }
}

fn skip_horizontal(bytes: &[u8], mut cursor: usize) -> usize {
    while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
        cursor += 1;
    }
    cursor
}

fn trim_horizontal(source: &str, range: TextRange) -> TextRange {
    let bytes = source.as_bytes();
    let mut start = range.start;
    let mut end = range.end;
    while matches!(bytes.get(start), Some(b' ' | b'\t')) {
        start += 1;
    }
    while end > start && matches!(bytes.get(end - 1), Some(b' ' | b'\t')) {
        end -= 1;
    }
    TextRange::new(start, end)
}

fn valid_fence_tail(bytes: &[u8], cursor: usize) -> bool {
    match bytes.get(cursor) {
        None | Some(b'\r' | b'\n') => true,
        Some(b' ' | b'\t') => {
            let tail = skip_horizontal(bytes, cursor);
            matches!(bytes.get(tail), None | Some(b'\r' | b'\n'))
        }
        Some(_) => false,
    }
}

fn trim_framing_newline(bytes: &[u8], end: usize, start: usize) -> usize {
    if end >= start + 2 && bytes.get(end - 2..end) == Some(b"\r\n") {
        end - 2
    } else if end > start && bytes.get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    }
}

pub(crate) fn containing(literals: &[RawLiteral], position: usize) -> Option<&RawLiteral> {
    literals
        .iter()
        .find(|literal| literal.range.start <= position && position < literal.range.end)
}

pub(crate) fn starting_at(literals: &[RawLiteral], position: usize) -> Option<&RawLiteral> {
    literals
        .iter()
        .find(|literal| literal.range.start == position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_fenced_raw_literals() {
        let source = "before `#[inline]`\n```not\n#code[[[raw]]]\n```\nafter";
        let parse = parse_raw_literals(source);
        let literals = parse.literals;
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(literals.len(), 2);
        assert_eq!(literals[0].form, RawLiteralForm::Inline);
        assert_eq!(
            &source[literals[0].payload_range.start..literals[0].payload_range.end],
            "#[inline]"
        );
        assert_eq!(literals[1].form, RawLiteralForm::Fenced);
        assert_eq!(literals[1].tag.as_ref().unwrap().value, "not");
        assert_eq!(
            &source[literals[1].payload_range.start..literals[1].payload_range.end],
            "#code[[[raw]]]"
        );
    }

    #[test]
    fn reports_unclosed_raw_literals_and_keeps_their_ranges_atomic() {
        let source = "before ```rust\nfn main() {}";
        let parse = parse_raw_literals(source);
        let literals = parse.literals;
        assert_eq!(parse.errors.len(), 1);
        assert!(
            parse.errors[0]
                .message
                .contains("unclosed fenced raw literal")
        );
        assert_eq!(literals[0].range.end, source.len());
        assert_eq!(literals[0].tag.as_ref().unwrap().value, "rust");
    }

    #[test]
    fn longer_inline_delimiters_can_contain_shorter_runs() {
        let source = "``one ` two``";
        let parse = parse_raw_literals(source);
        let literals = parse.literals;
        assert!(parse.errors.is_empty());
        assert_eq!(literals[0].delimiter_len, 2);
        assert_eq!(
            &source[literals[0].payload_range.start..literals[0].payload_range.end],
            "one ` two"
        );
    }

    #[test]
    fn unclosed_inline_literals_stop_at_newlines_and_scanning_continues() {
        let source = "`first\n`second`";
        let parse = parse_raw_literals(source);
        assert_eq!(parse.errors.len(), 1);
        assert_eq!(parse.literals.len(), 2);
        assert_eq!(parse.literals[0].range, TextRange::new(0, 6));
        assert_eq!(
            &source[parse.literals[0].payload_range.start..parse.literals[0].payload_range.end],
            "first"
        );
        assert_eq!(parse.literals[1].form, RawLiteralForm::Inline);
        assert_eq!(
            &source[parse.literals[1].payload_range.start..parse.literals[1].payload_range.end],
            "second"
        );
    }

    #[test]
    fn fenced_closer_must_end_its_line() {
        for suffix in [",", ")"] {
            let source = format!("```not\nfirst\n```{suffix}\nsecond\n```\n");
            let parse = parse_raw_literals(&source);
            assert!(parse.errors.is_empty(), "{:?}", parse.errors);
            assert_eq!(parse.literals.len(), 1);
            assert_eq!(
                &source[parse.literals[0].payload_range.start..parse.literals[0].payload_range.end],
                format!("first\n```{suffix}\nsecond")
            );
        }
    }
}
