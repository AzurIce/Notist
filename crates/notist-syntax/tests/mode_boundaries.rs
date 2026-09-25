use notist_syntax::{ExprKind, parse_source};

#[test]
fn code_and_markup_can_nest_across_both_file_entries() {
    for (path, source) in [
        (
            "nested.notc",
            "let name = \"N\"; [before #wrap[inside #name] after];",
        ),
        ("nested.not", "before #wrap[inside #text(\"N\")] after"),
    ] {
        let parsed = parse_source(path, source);
        assert!(parsed.errors.is_empty(), "{path}: {:?}", parsed.errors);
        let expressions = parsed.expressions();
        assert!(
            expressions
                .iter()
                .filter(|e| matches!(e.kind, ExprKind::Content(_)))
                .count()
                >= 1,
            "{path}: expected a content literal"
        );
        assert!(
            expressions
                .iter()
                .any(|e| matches!(e.kind, ExprKind::Call(..))),
            "{path}: expected an interpolated call"
        );
        assert!(
            expressions
                .iter()
                .all(|e| e.offset <= e.end && e.end <= source.len())
        );
        assert!(parsed.tokens.iter().any(|t| t.kind == "markup-interp"));
        assert!(parsed.tokens.iter().any(|t| t.kind == "name"));
    }
}
