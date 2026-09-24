use notist_analysis::EvaluationSession;
use notist_html::RenderHtml;
use notist_ir::{Content, ItemIndex, Value};
use notist_model::DiagnosticCode;

fn run(source: &str) -> notist_eval::Evaluation {
    let mut session = EvaluationSession::default();
    session.sources.insert("README.not".into(), source.into());
    session.evaluate("README.not")
}
fn items<'a>(content: &'a Content, name: &str) -> Vec<&'a Content> {
    let index = ItemIndex::new(content);
    (0..index.nodes().len())
        .map(|id| index.item(content, id))
        .filter(|item| item.name == name)
        .collect()
}
fn named<'a>(content: &'a Content, label: &str) -> &'a Content {
    content.label_matches(&[label.into()])[0].item
}

#[test]
fn core_structural_elements_form_and_render_nested_content() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "core.notc".into(),
        include_str!("../../../examples/packages/core/docs/README.notc").into(),
    );
    session.sources.insert("README.notc".into(), r#"
        use vault::core::{callout, details, quote, figure, image, table_cell, table, strike, underline};
        callout(kind: "tip", title: [Hint])[A #underline[word] and #strike[old]];
        details(summary: [More], open: true)[Expanded];
        quote(attribution: [Author])[Quoted];
        figure(caption: [Caption])[#image("pic.png", alt: "Picture")];
        table(2, header: true, align: "left,right")[#table_cell[Head A] #table_cell[Head B] #table_cell[Cell

        - nested] #table_cell[Last]];
    "#.into());
    let result = session.evaluate("README.notc");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(items(&result.content, "table-cell").len(), 4);
    assert!(!items(&result.content, "list-item").is_empty());
    let html = result.content.html();
    for expected in [
        "<aside",
        "<details",
        "<blockquote",
        "<figure",
        "<img",
        "<table",
        "<th",
        "<td",
        "<u>",
        "<s>",
        "text-align:left",
        "text-align:right",
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
}

#[test]
fn table_rejects_non_cells_and_incomplete_rows() {
    for body in [
        "[#text(\"plain\")]",
        "[#item(\"table-cell\", (body: [only one]))]",
    ] {
        let source = format!("item(\"table\", (columns: 2, body: {body}));");
        let mut session = EvaluationSession::default();
        session.sources.insert("README.notc".into(), source);
        let result = session.evaluate("README.notc");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::ContentConstraint),
            "{:?}",
            result.diagnostics
        );
    }
}

#[test]
fn table_spans_fill_successive_rows() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "core.notc".into(),
        include_str!("../../../examples/packages/core/docs/README.notc").into(),
    );
    session.sources.insert("README.notc".into(), "use vault::core::{table, table_cell}; table(2)[#table_cell(rowspan: 2)[A]#table_cell[B]#table_cell[C]];".into());
    let result = session.evaluate("README.notc");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let html = result.content.html();
    assert_eq!(html.matches("<tr>").count(), 2, "{html}");
    assert!(html.contains("rowspan=\"2\""), "{html}");
}

#[test]
fn image_rejects_non_web_sources() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "README.notc".into(),
        "item(\"image\", (source: \"javascript:alert(1)\", alt: \"x\"));".into(),
    );
    let html = session.evaluate("README.notc").content.html();
    assert!(html.contains("unsupported image source"), "{html}");
}

#[test]
fn literal_and_function_results_share_content_type_without_call_wrappers() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "main.notc".into(),
        r#"
        let empty = [];
        let single = [x];
        let multi = [x #text("y")];
        let foo = (body: Content) => item("paragraph", (body: body));
        let result = foo(single);
        result;
    "#
        .into(),
    );
    let (output, env) = session.evaluate_with_env("main.notc");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for name in ["empty", "single", "multi"] {
        let Value::Content(content) = &env[name] else {
            panic!("{name}")
        };
        assert_eq!(content.name, "seq");
        assert_eq!(env[name].ty(), notist_model::Type::Content);
    }
    assert_eq!(items(&output.content, "paragraph").len(), 1);
    assert!(items(&output.content, "foo").is_empty());
    assert!(!items(&output.content, "text").is_empty());
}

#[test]
fn paragraph_annotations_are_resolved_after_evaluation() {
    let result = run("@(label: \"whole\")\none two\nthree");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(items(&result.raw_content, "annotation").len(), 1);
    assert!(items(&result.raw_content, "paragraph").is_empty());
    assert_eq!(named(&result.content, "whole").name, "paragraph");
    assert_eq!(items(&result.content, "paragraph").len(), 1);
    assert!(items(&result.content, "annotation").is_empty());
    assert_eq!(items(&result.content, "space").len(), 2);
}

#[test]
fn adjacent_annotations_merge_but_explicit_sequences_keep_distinct_targets() {
    let merged = run("@(label: \"merged\", priority: 1) @(priority: 2) text");
    assert!(merged.diagnostics.is_empty());
    let paragraph = named(&merged.content, "merged");
    assert_eq!(paragraph.name, "paragraph");
    assert_eq!(paragraph.attributes["priority"].to_json(), 2);
    let nested = run("@(label: \"outer\") #[@(label: \"inner\") text]");
    assert!(nested.diagnostics.is_empty(), "{:?}", nested.diagnostics);
    assert_eq!(named(&nested.content, "outer").name, "seq");
    assert_eq!(named(&nested.content, "inner").name, "text");
    assert_eq!(
        nested
            .content
            .label_matches(&["outer".into(), "inner".into()])
            .len(),
        1
    );
}

#[test]
fn mixed_sequence_keeps_its_group_and_splits_outer_flow() {
    let result = run("a #[b\n\nc] d");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let children = result.content.children().collect::<Vec<_>>();
    assert_eq!(
        children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["paragraph", "seq", "paragraph"]
    );
    assert_eq!(
        children[1]
            .children()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["paragraph", "paragraph"]
    );
}

#[test]
fn reused_content_has_independent_annotation_targets() {
    let result =
        run("#let shared = [text];\n@(label: \"left\") #shared\n\n@(label: \"right\") #shared");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(named(&result.content, "left").name, "seq");
    assert_eq!(named(&result.content, "right").name, "seq");
    assert!(!std::ptr::eq(
        named(&result.content, "left"),
        named(&result.content, "right")
    ));
    assert!(result.raw_content.labeled_items().is_empty());
}

#[test]
fn explicit_paragraph_reports_invalid_content_and_retains_one_paragraph() {
    for body in ["a\n\nb", "a #item(\"block\", (:)) b"] {
        let source = format!("#item(\"paragraph\", (body: [{body}]))");
        let result = run(&source);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::ContentConstraint),
            "{source}"
        );
        assert_eq!(items(&result.content, "paragraph").len(), 1);
        assert!(!items(&result.content, "error").is_empty());
    }
}

#[test]
fn external_models_control_participation_and_slots_independently() {
    let result = run(
        "#define_element(\"badge\", true, (body: \"inline\"));\n#let badge = (body: Content) => item(\"badge\", (body: body));\na #badge[b] c",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(items(&result.content, "paragraph").len(), 1);
    assert_eq!(items(&result.content, "badge").len(), 1);
    let unknown = run("a #item(\"unknown\", (:)) b");
    assert_eq!(
        unknown
            .content
            .children()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["paragraph", "unknown", "paragraph"]
    );
    let nested = run(
        "#define_element(\"box\", true, (body: \"flow\"));\na #item(\"box\", (body: [b\n\nc])) d",
    );
    assert!(nested.diagnostics.is_empty(), "{:?}", nested.diagnostics);
    assert_eq!(nested.content.children().count(), 1);
    assert_eq!(items(&nested.content, "paragraph").len(), 3);
}

#[test]
fn entries_are_nested_items_with_main_and_detail_blocks() {
    let result = run("- Main\n\n  Detail\n\n  - Nested\n- Second");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(items(&result.content, "list").is_empty());
    let entries = items(&result.content, "list-item");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].main_content().unwrap().name, "paragraph");
    assert_eq!(
        entries[0]
            .detail_content()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["paragraph", "list-item"]
    );
    assert_eq!(result.content.children().count(), 2);
}

#[test]
fn formation_is_stable_and_does_not_mutate_raw_content() {
    let result = run("@(label: \"s\") #[a\n\nb]\n\n- main\n  - nested");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let formed = notist_ir::document::DocumentRules::default().form(&result.content);
    assert!(formed.diagnostics.is_empty());
    assert_eq!(formed.content.to_json(), result.content.to_json());
    assert!(!items(&result.raw_content, "annotation").is_empty());
}

#[test]
fn orphan_annotations_and_invalid_external_models_are_diagnostics() {
    for source in [
        "@(label: \"orphan\")",
        "#[@(label: \"inner\")] after",
        "#define_element(\"badge\", true, (body: \"invalid\"));",
        "#define_element(\"text\", false, (:));",
        "#with_attributes(item(\"annotation\", (attributes: (label: \"target\"))), (label: \"control\")) text",
    ] {
        let result = run(source);
        assert!(!result.diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn annotations_at_mid_flow_empty_groups_and_boundaries_have_explicit_targets() {
    let result = run(
        "before @(label: \"after\") next words\n\n@(label: \"empty\") #[]\n\n@(label: \"cross\")\n\nlast",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(items(&result.content, "paragraph").len(), 3);
    assert_eq!(named(&result.content, "after").name, "paragraph");
    assert_eq!(named(&result.content, "empty").name, "seq");
    assert_eq!(named(&result.content, "empty").children().count(), 0);
    assert_eq!(named(&result.content, "cross").name, "paragraph");
}

#[test]
fn source_package_models_are_loaded_with_imports_and_classify_actual_fields() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "library.notc".into(),
        r#"
        define_element("formula", true, (body: "inline"), "block");
        let formula = (block: Bool, body: Content) => item("formula", (block: block, body: body));
    "#
        .into(),
    );
    session.sources.insert(
        "README.not".into(),
        "#use vault::library::formula;\na #formula(false)[x] b\n\nc #formula(true)[y] d".into(),
    );
    let result = session.evaluate("README.not");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result
            .content
            .children()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["paragraph", "paragraph", "formula", "paragraph"]
    );
    assert_eq!(items(&result.content, "formula").len(), 2);
}

#[test]
fn formed_paragraph_and_reused_literal_retain_source_ranges() {
    let source = "@(label: \"whole\")\none two\nthree";
    let result = run(source);
    let paragraph = named(&result.content, "whole");
    let span = paragraph.span.as_ref().unwrap();
    assert_eq!(&source[span.start..span.end], "one two\nthree");
    assert_eq!(
        paragraph.origin.as_ref().unwrap().kind,
        notist_model::OriginKind::Formation
    );
    let source = "#let shared = [original];\n@(label: \"use\") #shared";
    let result = run(source);
    let span = named(&result.content, "use").span.as_ref().unwrap();
    assert_eq!(&source[span.start..span.end], "[original]");
}

#[test]
fn native_projection_preserves_leaf_attributes_and_nested_list_structure() {
    use notist_html::RenderHtml;
    let result = run(
        "#with_attributes(text(\"word\"), (label: \"word\")) #with_attributes(item(\"smartquote\", (double: true, open: true)), (label: \"quote\"))\n\n- First\n  - Nested\n- Second",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let html = result.content.html();
    assert!(html.contains("data-notist-label=\"word\""), "{html}");
    assert!(html.contains("data-notist-label=\"quote\""), "{html}");
    assert!(
        html.contains(
            "<ul><li><p>First</p><ul><li><p>Nested</p></li></ul></li><li><p>Second</p></li></ul>"
        ),
        "{html}"
    );
    let mut error = Content::error(
        "test",
        DiagnosticCode::Evaluation,
        notist_model::Location {
            source: "README.not".into(),
            offset: 0,
        },
    );
    error.apply_attributes(std::collections::BTreeMap::from([(
        "label".into(),
        Value::String("failure".into()),
    )]));
    assert!(error.html().contains("data-notist-label=\"failure\""));
}
