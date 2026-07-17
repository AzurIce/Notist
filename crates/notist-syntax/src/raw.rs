use notist_model::TextRange;

use crate::SyntaxError;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_literals(source: &str) -> (Vec<RawLiteral>, Vec<SyntaxError>) {
        let parse = crate::parse(source);
        (
            parse.raw_literals().into_iter().cloned().collect(),
            parse.errors,
        )
    }

    #[test]
    fn parses_inline_and_fenced_raw_literals() {
        let source = "before `#[inline]`\n```not\n#code[[[raw]]]\n```\nafter";
        let (literals, errors) = raw_literals(source);
        assert!(errors.is_empty(), "{:?}", errors);
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
        let (literals, errors) = raw_literals(source);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unclosed fenced raw literal"));
        assert_eq!(literals[0].range.end, source.len());
        assert_eq!(literals[0].tag.as_ref().unwrap().value, "rust");
    }

    #[test]
    fn longer_inline_delimiters_can_contain_shorter_runs() {
        let source = "``one ` two``";
        let (literals, errors) = raw_literals(source);
        assert!(errors.is_empty());
        assert_eq!(literals[0].delimiter_len, 2);
        assert_eq!(
            &source[literals[0].payload_range.start..literals[0].payload_range.end],
            "one ` two"
        );
    }

    #[test]
    fn unclosed_inline_literals_stop_at_newlines_and_scanning_continues() {
        let source = "`first\n`second`";
        let (literals, errors) = raw_literals(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(literals.len(), 2);
        assert_eq!(literals[0].range, TextRange::new(0, 6));
        assert_eq!(
            &source[literals[0].payload_range.start..literals[0].payload_range.end],
            "first"
        );
        assert_eq!(literals[1].form, RawLiteralForm::Inline);
        assert_eq!(
            &source[literals[1].payload_range.start..literals[1].payload_range.end],
            "second"
        );
    }

    #[test]
    fn fenced_closer_must_end_its_line() {
        for suffix in [",", ")"] {
            let source = format!("```not\nfirst\n```{suffix}\nsecond\n```\n");
            let (literals, errors) = raw_literals(&source);
            assert!(errors.is_empty(), "{:?}", errors);
            assert_eq!(literals.len(), 1);
            assert_eq!(
                &source[literals[0].payload_range.start..literals[0].payload_range.end],
                format!("first\n```{suffix}\nsecond")
            );
        }
    }
}
