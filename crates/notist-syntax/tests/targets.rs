use notist_model::Target;
use notist_syntax::{Expr, ExprKind, Statement, parse_source};

fn expression(source: &str) -> Expr {
    let parsed = parse_source("test.notc", source);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let [Statement::Expression(expr)] = parsed.statements.as_slice() else {
        panic!("expected one expression: {:?}", parsed.statements);
    };
    expr.clone()
}

fn wikilink(source: &str) -> Expr {
    let parsed = parse_source("test.not", source);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let [Statement::Expression(target)] = parsed.statements.as_slice() else {
        panic!("expected one raw target: {:?}", parsed.statements);
    };
    assert_eq!(target.offset, 0);
    assert_eq!(target.end, source.len());
    let tokens: Vec<_> = parsed
        .tokens
        .iter()
        .filter(|t| t.kind == "wikilink")
        .collect();
    assert_eq!(tokens.len(), 1);
    assert_eq!((tokens[0].start, tokens[0].end), (0, source.len()));
    target.clone()
}

#[test]
fn code_and_wikilinks_share_label_segment_parsing() {
    let cases = [
        Target::new("vault::guide", vec!["安装".into(), "例子".into()]),
        Target::new(
            "vault::指南",
            vec!["包括::分隔符".into(), "包含\"引号\"和]]与\\\n".into()],
        ),
    ];
    for target in cases {
        let code = expression(&format!("{target};"));
        let markup = wikilink(&format!("[[{target}]]"));
        for parsed in [code, markup] {
            let ExprKind::Target(module, labels) = parsed.kind else {
                panic!("expected a target");
            };
            assert_eq!(module.join("::"), target.module);
            assert_eq!(labels, target.labels);
        }
    }
}

#[test]
fn module_only_wikilink_has_no_label_constraints() {
    let ExprKind::Target(module, labels) = wikilink("[[vault::guide]]").kind else {
        panic!("expected a target");
    };
    assert_eq!(module, ["vault", "guide"]);
    assert!(labels.is_empty());
    assert!(matches!(
        expression("vault::guide;").kind,
        ExprKind::Name(name) if name == "vault::guide"
    ));
}

#[test]
fn invalid_label_paths_and_old_item_syntax_are_rejected() {
    for path in [
        "vault::guide#intro",
        "vault::guide::",
        "vault::guide::\"\"",
        "vault::guide::\"A\"::",
        "vault::guide::\"A\"::child",
        "vault::guide::\"A\"::\"\"",
        "vault::guide::\"unterminated",
        r#"vault::guide::"invalid\q""#,
    ] {
        for (file, source) in [
            ("test.notc", format!("{path};")),
            ("test.not", format!("[[{path}]]")),
        ] {
            let result = parse_source(file, &source);
            assert!(!result.errors.is_empty(), "accepted invalid path: {source}");
        }
    }
    for source in ["[[]]", "[[vault::guide::\"A\"] ]", "[[vault::guide"] {
        assert!(
            !parse_source("test.not", source).errors.is_empty(),
            "{source}"
        );
    }
}
