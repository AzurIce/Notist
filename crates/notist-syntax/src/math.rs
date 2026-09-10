use notist_model::TextRange;

use crate::{SpannedText, SyntaxError};

/// A `$...$` inline span or `$$`-fenced block of raw math source (math
/// sugar). The payload is verbatim source text: no Markup interpretation
/// happens inside, and `\$` stays in the payload without closing the span.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MathSpan {
    /// The math source payload without the surrounding delimiters.
    pub source: SpannedText,
    /// Whether the span is an inline `$...$` or a block fenced by `$$` lines.
    pub form: MathForm,
    /// The complete span range including delimiters.
    pub range: TextRange,
}

/// The source form of a math span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathForm {
    /// A `$...$` span delimited on one line.
    Inline,
    /// A block whose opener is a lone `$$` line, closed by a lone `$$` line.
    Block,
}

/// The outcome of scanning at a `$`.
pub(crate) enum MathScan {
    /// A math span, with an optional diagnostic when it was unclosed.
    Span(MathSpan, Option<SyntaxError>),
    /// A `$$` run that is not alone on its line: not block sugar.
    Misplaced(TextRange),
    /// Not math sugar: the `$` stays ordinary text.
    Plain,
}

pub(crate) fn scan_at(source: &str, start: usize, line_leading: bool) -> MathScan {
    let bytes = source.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'$'));

    let dollar_run_end = dollar_run_end(bytes, start);
    if dollar_run_end - start >= 2 {
        let line_end = find_line_end(bytes, dollar_run_end);
        let rest_blank = line_end
            .map(|end| skip_horizontal(bytes, dollar_run_end) == end)
            .unwrap_or(true);
        if line_leading && rest_blank {
            return match parse_block(source, start, dollar_run_end) {
                Some(result) => MathScan::Span(result.0, result.1),
                None => MathScan::Misplaced(TextRange::new(start, dollar_run_end)),
            };
        }
        return MathScan::Misplaced(TextRange::new(start, dollar_run_end));
    }
    match parse_inline(source, start) {
        Some(result) => MathScan::Span(result.0, result.1),
        None => MathScan::Plain,
    }
}

/// Scans a `$$`-fenced block. `None` when the opener line carries trailing
/// content — a block opener must stand alone on its line.
fn parse_block(
    source: &str,
    start: usize,
    opening_end: usize,
) -> Option<(MathSpan, Option<SyntaxError>)> {
    let bytes = source.as_bytes();
    let payload_start = match find_line_end(bytes, opening_end) {
        Some(line_end) => newline_end(bytes, line_end),
        None => bytes.len(),
    };
    let mut cursor = payload_start;

    while cursor < bytes.len() {
        let line_start = cursor;
        let content_start = skip_horizontal(bytes, line_start);
        let close_end = dollar_run_end(bytes, content_start);
        if close_end - content_start >= 2 && valid_close_tail(bytes, close_end) {
            let payload_end = trim_framing_newline(bytes, line_start, payload_start);
            return Some((
                MathSpan {
                    source: SpannedText {
                        value: source[payload_start..payload_end].to_owned(),
                        range: TextRange::new(payload_start, payload_end),
                    },
                    form: MathForm::Block,
                    range: TextRange::new(start, close_end),
                },
                None,
            ));
        }
        cursor = match find_line_end(bytes, line_start) {
            Some(end) => newline_end(bytes, end),
            None => bytes.len(),
        };
    }

    Some((
        MathSpan {
            source: SpannedText {
                value: source[payload_start..].to_owned(),
                range: TextRange::new(payload_start, source.len()),
            },
            form: MathForm::Block,
            range: TextRange::new(start, source.len()),
        },
        Some(SyntaxError {
            message: "unclosed block math span; expected a closing `$$` line".into(),
            range: TextRange::new(start, source.len()),
        }),
    ))
}

/// Scans an inline `$...$` span. `None` when the `$` does not open a span:
/// at end of file/line, before whitespace, or before a second `$`.
fn parse_inline(source: &str, start: usize) -> Option<(MathSpan, Option<SyntaxError>)> {
    let bytes = source.as_bytes();
    let opening_end = start + 1;
    if matches!(
        bytes.get(opening_end),
        None | Some(b'$' | b'\t' | b'\r' | b'\n' | b' ')
    ) {
        return None;
    }

    let line_end = find_line_end(bytes, opening_end).unwrap_or(bytes.len());
    let mut cursor = opening_end;
    while cursor < line_end {
        if bytes[cursor] == b'\\' && bytes.get(cursor + 1) == Some(&b'$') {
            cursor += 2;
            continue;
        }
        // The closer must hug content, mirroring the emphasis delimiters:
        // prose like "costs $5 and $10" then fails loudly instead of
        // silently typesetting the middle as math.
        if bytes[cursor] == b'$' && !bytes[cursor - 1].is_ascii_whitespace() {
            return Some((
                MathSpan {
                    source: SpannedText {
                        value: source[opening_end..cursor].to_owned(),
                        range: TextRange::new(opening_end, cursor),
                    },
                    form: MathForm::Inline,
                    range: TextRange::new(start, cursor + 1),
                },
                None,
            ));
        }
        cursor += 1;
    }

    Some((
        MathSpan {
            source: SpannedText {
                value: source[opening_end..line_end].to_owned(),
                range: TextRange::new(opening_end, line_end),
            },
            form: MathForm::Inline,
            range: TextRange::new(start, line_end),
        },
        Some(SyntaxError {
            message: "unclosed inline math span; expected a closing `$`".into(),
            range: TextRange::new(start, line_end),
        }),
    ))
}

fn dollar_run_end(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes.get(cursor) == Some(&b'$') {
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

fn valid_close_tail(bytes: &[u8], cursor: usize) -> bool {
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

    fn spans(source: &str) -> Vec<MathSpan> {
        collect(source).0
    }

    fn errors(source: &str) -> Vec<SyntaxError> {
        collect(source).1
    }

    /// Scans every `$` occurrence like the parser dispatch would.
    fn collect(source: &str) -> (Vec<MathSpan>, Vec<SyntaxError>) {
        let bytes = source.as_bytes();
        let mut spans = Vec::new();
        let mut errors = Vec::new();
        let mut cursor = 0;
        let mut line_leading = true;
        while cursor < bytes.len() {
            if bytes[cursor] == b'$' {
                match scan_at(source, cursor, line_leading) {
                    MathScan::Span(span, error) => {
                        spans.push(span.clone());
                        if let Some(error) = error {
                            errors.push(error);
                        }
                        line_leading = false;
                        cursor = span.range.end.max(cursor + 1);
                    }
                    MathScan::Misplaced(range) => {
                        errors.push(SyntaxError {
                            message: "block math delimiter `$$` must stand alone on its line"
                                .into(),
                            range,
                        });
                        line_leading = false;
                        cursor += 2;
                    }
                    MathScan::Plain => {
                        line_leading = false;
                        cursor += 1;
                    }
                }
                continue;
            }
            line_leading = match bytes[cursor] {
                b' ' | b'\t' => line_leading,
                b'\n' => true,
                _ => false,
            };
            cursor += 1;
        }
        (spans, errors)
    }

    fn payload<'a>(source: &'a str, span: &MathSpan) -> &'a str {
        &source[span.source.range.start..span.source.range.end]
    }

    #[test]
    fn parses_inline_spans() {
        let source = "Euler: $e^{i\\pi} + 1 = 0$ ok";
        let spans = spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].form, MathForm::Inline);
        assert_eq!(payload(source, &spans[0]), "e^{i\\pi} + 1 = 0");
    }

    #[test]
    fn escaped_dollar_stays_in_payload_without_closing() {
        let source = "$a \\$ b$ tail";
        let spans = spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(payload(source, &spans[0]), "a \\$ b");
    }

    #[test]
    fn opener_and_closer_need_hugging_content() {
        // `$ x$` never opens; prose dollars fail loudly instead of
        // typesetting the middle of the sentence as math.
        assert!(errors("$ x$").is_empty());
        let prose = "costs $5 and $10 here";
        assert_eq!(errors(prose).len(), 1);
        assert!(errors(prose)[0].message.contains("unclosed inline math"));
        assert!(spans("$").is_empty());
    }

    #[test]
    fn unclosed_inline_degrades_to_line_end_with_diagnostic() {
        let source = "value $x^2\nnext line";
        let errors = errors(source);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unclosed inline math"));
        let spans = spans(source);
        assert_eq!(payload(source, &spans[0]), "x^2");
    }

    #[test]
    fn math_payload_is_opaque_to_markup() {
        let source = "$a * b_1#not$ tail";
        let spans = spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(payload(source, &spans[0]), "a * b_1#not");
    }

    #[test]
    fn parses_block_spans() {
        let source = "before\n$$\na^2 + b^2\n= c^2\n$$\nafter";
        let errors = errors(source);
        assert!(errors.is_empty(), "{errors:?}");
        let spans = spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].form, MathForm::Block);
        assert_eq!(payload(source, &spans[0]), "a^2 + b^2\n= c^2");
    }

    #[test]
    fn block_opener_must_stand_alone() {
        // Trailing content on the opener line is not block sugar: one
        // diagnostic, and the rest of the line stays ordinary text.
        let source = "$$ x\ny\n";
        let found = spans(source);
        assert!(found.is_empty());
        let diagnostics = errors(source);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("must stand alone"));
        // `$$` not at a line start is not block sugar either; the delimiters
        // degrade to ordinary text with a diagnostic each.
        let source = "text $$ x $$";
        assert!(spans(source).is_empty());
        assert_eq!(errors(source).len(), 2);
    }

    #[test]
    fn block_closer_may_carry_trailing_whitespace() {
        let source = "$$\nx\n$$   \nafter";
        let errors = errors(source);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(payload(source, &spans(source)[0]), "x");
    }

    #[test]
    fn unclosed_block_eats_to_eof_with_diagnostic() {
        let source = "$$\nx\ny";
        let errors = errors(source);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unclosed block math"));
        assert_eq!(payload(source, &spans(source)[0]), "x\ny");
    }

    #[test]
    fn block_survives_inside_list_rows_like_raw() {
        let source = "- item\n  $$\n  x^2\n  $$\n- next";
        let errors = errors(source);
        assert!(errors.is_empty(), "{errors:?}");
        let spans = spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].form, MathForm::Block);
    }
}
