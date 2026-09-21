use notist_analysis::EvaluationSession;
use notist_eval::Evaluation;
use notist_html::RenderHtml;

fn evaluate(source: &str) -> Evaluation {
    let path = std::env::var_os("NOTIST_SDK_WASM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/release/examples/semantic.wasm")
        });
    let mut runtime = EvaluationSession::default();
    runtime.binaries.insert(
        "semantic.wasm".into(),
        std::fs::read(path).expect("build the SDK example first: just test-plugin-sdk"),
    );
    runtime.sources.insert(
        "README.notc".into(),
        format!("wasm \"semantic.wasm\"; {source}"),
    );
    runtime.evaluate("README.notc")
}

#[test]
#[ignore = "requires compiled Rust plugin; run just test-plugin-sdk"]
fn rust_plugin_shares_language_calling_semantics() {
    let result = evaluate(
        r#"
        echo(source: "hello"); echo(); str(add(right: 4, left: 3));
        str(optional(9)); if optional() == none { "empty" } else { "wrong" };
        paragraph()[body]; paragraph(item("strong", (body: [nested])));
        let data = identity((key: (1, true, none), body: [content]));
        data.body;
    "#,
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.content.html(),
        "hellodefault79empty<p>body</p><p><strong>nested</strong></p>content"
    );
}

#[test]
#[ignore = "requires compiled Rust plugin; run just test-plugin-sdk"]
fn rust_plugin_errors_remain_at_the_call_site() {
    for (source, expected) in [
        ("add(left: true);", "expects Int"),
        ("add();", "missing argument"),
        ("add(1, left: 2);", "duplicate argument"),
        ("add(1, none);", "expects Int"),
        ("identity();", "missing argument"),
        ("optional(true);", "expects Optional"),
        ("optional(none, value: 1);", "duplicate argument"),
        ("identity((callback: () => 1));", "cannot cross"),
        ("checked(-1);", "expected nonnegative value"),
    ] {
        let result = evaluate(source);
        assert_eq!(result.warnings.len(), 1, "{source}: {:?}", result.warnings);
        assert!(
            result.warnings[0].contains(expected),
            "{source}: {:?}",
            result.warnings
        );
        assert!(result.warnings[0].starts_with("README.notc:"));
    }
}

#[test]
#[ignore = "requires compiled Rust plugin; run just test-plugin-sdk"]
fn rust_plugin_and_source_share_optional_default_precedence() {
    let result = evaluate(
        r#"
        let show = (x: Int?) => if x == none { "none" } else { str(x) };
        show(optional()); show(optional(none)); show(optional(value: 3));
        show(preferred()); show(preferred(none)); show(preferred(value: 5));
    "#,
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.html(), "nonenone32none5");
}

#[test]
#[ignore = "requires compiled Rust plugin; run just test-plugin-sdk"]
fn plugin_origins_survive_indirect_calls_without_entering_the_wire_format() {
    let result = evaluate("map(paragraph, ([One], [Two]));");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let tree = notist_ir::ItemIndex::new(&result.content);
    let paragraphs: Vec<_> = tree
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(id, _)| {
            let item = tree.item(&result.content, id);
            (item.name == "paragraph").then_some(item)
        })
        .collect();
    assert_eq!(paragraphs.len(), 2);
    for item in paragraphs {
        let origin = item.origin.as_ref().unwrap();
        assert_eq!(origin.kind, notist_model::OriginKind::Plugin);
        assert!(origin.node_id.is_some());
        let wire = notist_ir::Value::Item(item.clone()).to_abi().unwrap();
        assert!(
            !serde_json::to_value(wire)
                .unwrap()
                .to_string()
                .contains("node_id")
        );
    }
}
