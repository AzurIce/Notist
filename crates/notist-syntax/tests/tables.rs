use notist_syntax::{ExprKind, Statement, parse_source};

#[test]
fn pipe_table_lowers_to_existing_table_and_cell_nodes() {
    let source = "| Name | Value |\n| :--- | ---: |\n| A\\|B | *two* |\n";
    for (path, text) in [
        ("table.not", source.to_owned()),
        ("table.notc", format!("[\n{source}];")),
    ] {
        let parsed = parse_source(path, &text);
        assert!(parsed.errors.is_empty(), "{path}: {:?}", parsed.errors);
        let table = parsed
            .expressions()
            .into_iter()
            .find_map(|e| match &e.kind {
                ExprKind::Element(name, fields) if name == "table" => Some(fields),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{path}: table node missing: {:?}", parsed.statements));
        assert!(
            table.iter().any(|(name, value)| {
                name == "columns" && matches!(value.kind, ExprKind::Int(2))
            })
        );
        assert!(table.iter().any(|(name, value)| {
            name == "align"
                && matches!(&value.kind, ExprKind::String(value) if value == "left,right")
        }));
        let (_, body) = table.iter().find(|(name, _)| name == "body").unwrap();
        let ExprKind::Content(cells) = &body.kind else {
            panic!("table body must be Content");
        };
        assert_eq!(cells.len(), 4);
        assert!(cells.iter().all(|cell| {
            matches!(&cell.kind, ExprKind::Element(name, _) if name == "table-cell")
        }));
        assert_eq!(
            parsed
                .tokens
                .iter()
                .filter(|token| token.kind == "table-separator")
                .count(),
            1
        );
    }
}

#[test]
fn ordinary_pipes_remain_text_and_mismatched_rows_report_an_error() {
    let plain = parse_source("plain.not", "A | B\nplain | text");
    assert!(plain.errors.is_empty());
    assert!(!plain.statements.iter().any(|statement| {
        matches!(statement, Statement::Expression(expr) if matches!(&expr.kind, ExprKind::Element(name, _) if name == "table"))
    }));

    let invalid = parse_source("invalid.not", "| A | B |\n| - | - |\n| one |\n");
    assert!(
        invalid
            .errors
            .iter()
            .any(|e| e.message.contains("table row width"))
    );
}
