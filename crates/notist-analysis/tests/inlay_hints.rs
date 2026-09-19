use notist_analysis::{Workspace, position};
use serde_json::json;

#[test]
fn hints_cover_inferred_bindings_and_returns_without_repeating_annotations() {
    for (uri, prefix) in [
        ("file:///tmp/hints.notc", ""),
        ("file:///tmp/hints.not", "#"),
    ] {
        let source = format!(
            "{prefix}let 文 = \"😀\";\n{prefix}let fixed: Int = 1;\n{prefix}let f = (x: Int) => x + 1;\n{prefix}let g = () -> String => \"yes\";\n{prefix}let unknown = missing;\n"
        );
        let mut ws = Workspace::default();
        ws.open(uri.into(), source.clone(), Some(1));
        assert!(ws.documents[uri].parsed.errors.is_empty());
        for utf8 in [false, true] {
            let requested =
                json!({"start":{"line":0,"character":0},"end":position(&source,source.len(),utf8)});
            let hints = ws.inlay_hints(uri, &requested, utf8);
            let hints = hints.as_array().unwrap();
            assert_eq!(hints.len(), 4, "{hints:?}");
            assert_eq!(hints[0]["label"], ": String");
            assert_eq!(
                hints[0]["position"],
                position(&source, source.find('文').unwrap() + '文'.len_utf8(), utf8)
            );
            assert!(hints.iter().any(|h| h["label"] == " -> Int"));
            assert!(!hints.iter().any(|h| h["label"] == " -> String"));
            assert!(
                ws.inlay_hints(
                    uri,
                    &json!({"start":{"line":1,"character":0},"end":{"line":2,"character":0}}),
                    utf8
                )
                .as_array()
                .unwrap()
                .is_empty()
            );
        }
    }
}

#[test]
fn static_annotation_errors_and_hover_use_declared_contracts() {
    let uri = "file:///tmp/hints.notc";
    let mut ws = Workspace::default();
    let source = "let x: Int = \"bad\";\nlet f = () -> String => 2;\nlet y: Int = missing;\nx;";
    ws.open(uri.into(), source.into(), Some(1));
    let diagnostics = ws.diagnostics(uri, false);
    assert_eq!(diagnostics.as_array().unwrap().len(), 2, "{diagnostics}");
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("expected Int, got String")
    );
    assert_eq!(
        ws.hover(
            uri,
            &position(source, source.rfind("x;").unwrap(), false),
            false
        )["contents"]["value"],
        "let x: Int"
    );
}
