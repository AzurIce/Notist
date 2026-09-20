use notist_analysis::{Workspace, position};

fn display(source: &str, needle: &str, markup: bool) -> String {
    let uri = if markup {
        "file:///tmp/types.not"
    } else {
        "file:///tmp/types.notc"
    };
    let mut ws = Workspace::default();
    ws.open(uri.into(), source.into(), Some(1));
    assert!(
        ws.documents[uri].parsed.errors.is_empty(),
        "{:?}",
        ws.documents[uri].parsed.errors
    );
    let hover = ws.hover(
        uri,
        &position(source, source.rfind(needle).unwrap(), false),
        false,
    );
    hover["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("missing hover: {source}"))
        .into()
}

#[test]
fn types_replace_initializer_source() {
    for (expr, expected) in [
        ("\"Notist packages\"", "String"),
        ("42", "Int"),
        ("true", "Bool"),
        ("none", "None"),
        ("[Hello]", "Content"),
        ("(1, 2)", "List"),
        ("(name: \"hi\")", "Dict"),
    ] {
        let source = format!("let title = {expr};\ntitle;");
        assert_eq!(
            display(&source, "title;", false),
            format!("let title: {expected}")
        );
        assert_eq!(
            display(&source, "title =", false),
            format!("let title: {expected}")
        );
    }
}

#[test]
fn types_follow_bindings_calls_fields_and_lexical_scope() {
    for (source, expected) in [
        (
            "let a = \"hello\"; let title = a; title;",
            "let title: String",
        ),
        (
            "let a = 1; let a = \"hi\"; let title = a; title;",
            "let title: String",
        ),
        (
            "let add = (x: Int) => x + 1; let title = add(2); title;",
            "let title: Int",
        ),
        (
            "let f = (x: Int) => x == 2; let g = f; let title = g(2); title;",
            "let title: Bool",
        ),
        (
            "let obj = (name: \"hi\"); let title = obj.name; title;",
            "let title: String",
        ),
        ("let title = text(\"hi\"); title;", "let title: Content"),
        ("let title = math(\"x\"); title;", "let title: Item"),
        (
            "let text = (x: String) => 3; let title = text(\"hi\"); title;",
            "let title: Int",
        ),
        (
            "let title = if true { 1 } else { \"hi\" }; title;",
            "let title: {unknown}",
        ),
        ("let title = missing(); title;", "let title: {unknown}"),
        (
            "let recurse = (x: Int) => recurse(x); let title = recurse(1); title;",
            "let title: {unknown}",
        ),
    ] {
        assert_eq!(display(source, "title;", false), expected, "{source}");
    }
    let source = "#let x = 1;\n= Nested\n#let title = x + 2;\n#title";
    assert_eq!(display(source, "title", true), "let title: Int");
}

#[test]
fn function_signatures_include_return_types_and_parameters_have_hover() {
    let source = "let x = \"outer\"; let add = (x: Int, y: Int = 2) => x + y; add;";
    assert_eq!(
        display(source, "add;", false),
        "let add: (x: Int, y: Int = 2) -> Int"
    );
    assert_eq!(display(source, "x +", false), "x: Int");
    assert_eq!(display(source, "y;", false), "y: Int");
    assert_eq!(
        display("let f = (x) => x; f;", "f;", false),
        "let f: (x: Any) -> Any"
    );
}

#[test]
fn optional_parameter_hover_preserves_type_and_explicit_defaults() {
    let source = "let f = (x: Int?, y: Int? = 2) -> Int? => x; f();";
    assert_eq!(
        display(source, "f();", false),
        "let f: (x: Int?, y: Int? = 2) -> Int?"
    );
    assert_eq!(display(source, "x;", false), "Int?");
}
