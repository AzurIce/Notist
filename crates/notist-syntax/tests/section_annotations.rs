use notist_syntax::{ExprKind, Statement, parse_source};

#[test]
fn leading_item_annotations_follow_sibling_and_ancestor_headings() {
    for preceding in ["= First\nText\n", "= First\n== Nested\nText\n"] {
        let source = format!(
            "{preceding}@(label: \"second\")\n// Heading metadata\n@(other: 1)\n= Second\nBody"
        );
        let parsed = parse_source("test.not", &source);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let [
            Statement::Expression(first),
            Statement::Expression(label),
            Statement::Expression(other),
            Statement::Expression(second),
        ] = parsed.statements.as_slice()
        else {
            panic!("unexpected statement structure: {:?}", parsed.statements);
        };
        assert!(matches!(first.kind, ExprKind::Section(..)));
        assert_eq!(first.end, preceding.len());
        assert!(matches!(label.kind, ExprKind::Annotation(false, _)));
        assert!(matches!(other.kind, ExprKind::Annotation(false, _)));
        assert!(matches!(second.kind, ExprKind::Section(..)));
        assert_eq!(
            parsed
                .tokens
                .iter()
                .filter(|token| token.kind == "annotation")
                .count(),
            2
        );
        assert_eq!(
            parsed
                .tokens
                .iter()
                .filter(|token| token.kind == "comment")
                .count(),
            1
        );
    }
}

#[test]
fn child_heading_annotations_are_separate_from_the_preceding_paragraph() {
    let parsed = parse_source(
        "test.not",
        "= Parent\nParagraph\n@(label: \"child\")\n== Child\nBody",
    );
    assert!(parsed.errors.is_empty());
    let [Statement::Expression(parent)] = parsed.statements.as_slice() else {
        panic!("missing parent");
    };
    let ExprKind::Section(_, _, children) = &parent.kind else {
        panic!("missing section");
    };
    assert_eq!(children.len(), 3);
    assert!(matches!(children[0].kind, ExprKind::Element(..)));
    assert!(matches!(children[1].kind, ExprKind::Annotation(false, _)));
    assert!(matches!(children[2].kind, ExprKind::Section(..)));
}

#[test]
fn module_annotations_keep_their_original_section_scope() {
    let parsed = parse_source("test.not", "= First\n@!(title: \"module\")\n= Second\nBody");
    assert!(parsed.errors.is_empty());
    let [Statement::Expression(first), Statement::Expression(_)] = parsed.statements.as_slice()
    else {
        panic!("expected sections");
    };
    let ExprKind::Section(_, _, children) = &first.kind else {
        panic!("missing section");
    };
    assert!(matches!(children[0].kind, ExprKind::Annotation(true, _)));
}
