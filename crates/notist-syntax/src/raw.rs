use notist_model::TextRange;

pub(crate) fn raw_ranges(source: &str) -> Vec<TextRange> {
    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }

        let delimiter_start = cursor;
        while bytes.get(cursor) == Some(&b'`') {
            cursor += 1;
        }
        let delimiter_len = cursor - delimiter_start;

        let mut search = cursor;
        let mut closing_end = None;
        while search < bytes.len() {
            if bytes[search] != b'`' {
                search += 1;
                continue;
            }
            let closing_start = search;
            while bytes.get(search) == Some(&b'`') {
                search += 1;
            }
            if search - closing_start >= delimiter_len {
                closing_end = Some(search);
                break;
            }
        }

        if let Some(end) = closing_end {
            ranges.push(TextRange::new(delimiter_start, end));
            cursor = end;
        }
    }

    ranges
}

pub(crate) fn containing(ranges: &[TextRange], position: usize) -> Option<TextRange> {
    ranges
        .iter()
        .copied()
        .find(|range| range.start <= position && position < range.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_and_fenced_raw_ranges() {
        let source = "before `#[inline]`\n```not\n#code[[[raw]]]\n```\nafter";
        let ranges = raw_ranges(source);
        assert_eq!(ranges.len(), 2);
        assert_eq!(&source[ranges[0].start..ranges[0].end], "`#[inline]`");
        assert_eq!(
            &source[ranges[1].start..ranges[1].end],
            "```not\n#code[[[raw]]]\n```"
        );
    }
}
