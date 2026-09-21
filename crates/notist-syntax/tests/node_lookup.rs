use notist_syntax::parse_source;

#[test]
fn node_lookup_matches_all_expression_branches_and_rejects_statement_ids() {
    for (name, source) in [
        (
            "README.notc",
            r#"
            let f = (a: Int = 2, b: Int, c = 4) => if a > b { a + c } else { b };
            let data = (nested: ((1, 2), (:)), content: [*Bold* _em_]);
            f(1, b: 3); data.nested; self::"Label";
            [#let nested = 3; #nested];
        "#,
        ),
        (
            "README.not",
            "@!(title: \"module\")\n@(label: \"Label\")\n= Title\n*Strong* _em_ $math$ `raw`\n== Child\n#text(\"hi\")\n= Sibling\n[[self::\"Label\"]]",
        ),
    ] {
        let parsed = parse_source(name, source);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let all = parsed.expressions();
        for id in 0..=parsed.id_count + 1 {
            let expected = all.iter().copied().find(|e| e.id == id);
            let actual = parsed.expression(id);
            assert_eq!(
                actual.map(|e| e.id),
                expected.map(|e| e.id),
                "{name}, id={id}"
            );
            if let (Some(actual), Some(expected)) = (actual, expected) {
                assert!(std::ptr::eq(actual, expected));
            }
        }
    }
}
