use notist_analysis::{Workspace, file_uri, position, range};
use serde_json::Value;
use std::fs;

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("docs")).unwrap();
    fs::write(
        temp.path().join("Notist.toml"),
        "[package]\nname = 'labels'\n",
    )
    .unwrap();
    for (name, text) in files {
        let file = temp.path().join("docs").join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut workspace = Workspace::default();
    workspace.load(temp.path());
    assert!(workspace.projects.errors.is_empty());
    for doc in workspace.documents.values() {
        assert!(doc.parsed.errors.is_empty(), "{:?}", doc.parsed.errors);
    }
    (temp, workspace)
}

fn definition(workspace: &Workspace, uri: &str, needle: &str, utf8: bool) -> Value {
    let source = &workspace.documents[uri].text;
    workspace.definition(
        uri,
        &position(source, source.find(needle).unwrap(), utf8),
        utf8,
    )
}

#[test]
fn definitions_use_evaluated_titles_explicit_labels_and_ordered_ancestors() {
    let source = concat!(
        "[[vault::guide::\"A\"::\"例子😀\"]]\n\n",
        "[[vault::guide::\"固定\"]]\n\n",
        "[[vault::guide::\"段落\"]]\n\n",
        "[[vault::guide]]",
    );
    let guide = concat!(
        "#let title = \"例子😀\";\n",
        "= A\n== Middle\n=== #title\nBody\n",
        "= B\n== #title\nOther\n",
        "@(label: \"固定\")\n= Display title\n",
        "@(label: \"段落\")\nSome text.",
    );
    let (temp, mut workspace) = project(&[("README.not", source), ("guide.not", guide)]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    let guide_uri = file_uri(&temp.path().join("docs/guide.not").canonicalize().unwrap());
    let snapshot = workspace.snapshot();
    let guide_source = snapshot
        .sources()
        .keys()
        .find(|source| snapshot.origin(source) == Some(guide_uri.as_str()))
        .unwrap();
    let evaluated = snapshot.evaluate(guide_source);
    assert!(evaluated.warnings.is_empty(), "{:?}", evaluated.warnings);
    assert_eq!(
        evaluated
            .content
            .label_matches(&["A".into(), "例子😀".into()])
            .len(),
        1,
        "{:?}",
        evaluated.content
    );
    for utf8 in [false, true] {
        let target = definition(&workspace, &uri, "例子😀", utf8);
        let start = guide.find("=== #title").unwrap();
        assert_eq!(target["uri"], guide_uri);
        assert_eq!(
            target["range"],
            range(guide, start, start + "=== #title".len(), utf8)
        );
        for (label, text) in [("固定", "= Display title"), ("段落", "Some text.")] {
            let target = definition(&workspace, &uri, label, utf8);
            assert_eq!(target["uri"], guide_uri);
            assert_eq!(
                target["range"]["start"],
                position(guide, guide.find(text).unwrap(), utf8)
            );
        }
        let module = definition(&workspace, &uri, "[[vault::guide]]", utf8);
        assert_eq!(module["uri"], guide_uri);
        assert_eq!(module["range"], range(guide, 0, 0, utf8));
        assert_eq!(workspace.diagnostics(&uri, utf8), serde_json::json!([]));
    }
    workspace.open(guide_uri, guide.replace("例子😀", "renamed"), Some(2));
    assert!(definition(&workspace, &uri, "例子😀", false).is_null());
    assert!(
        workspace
            .diagnostics(&uri, false)
            .to_string()
            .contains("no matching Item")
    );
}

#[test]
fn missing_and_ambiguous_paths_have_diagnostics_and_never_pick_a_candidate() {
    let source = concat!(
        "[[vault::guide::\"例子\"]]\n\n",
        "[[vault::guide::\"Absent\"]]\n\n",
        "[[vault::guide::\"例子\"::\"A\"]]\n\n",
        "[[vault::absent::\"A\"]]",
    );
    let guide = "= A\n== 例子\nOne\n= B\n== 例子\nTwo";
    let (temp, workspace) = project(&[("README.not", source), ("guide.not", guide)]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    for (index, reference) in source.split("\n\n").enumerate() {
        assert!(definition(&workspace, &uri, reference, false).is_null());
        let diagnostic = workspace.diagnostics(&uri, false);
        let entries = diagnostic.as_array().unwrap();
        assert_eq!(entries.len(), 4, "{diagnostic}");
        assert_eq!(entries[index]["severity"], 1);
        let start = source.find(reference).unwrap();
        assert_eq!(
            entries[index]["range"],
            range(source, start, start + reference.len(), false)
        );
    }
    let diagnostics = workspace.diagnostics(&uri, false);
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("ambiguous LabelPath")
    );
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("2 matching Items")
    );
    assert!(
        diagnostics[3]["message"]
            .as_str()
            .unwrap()
            .contains("missing target module")
    );
}

#[test]
fn definition_preserves_lexical_module_values_and_rejects_label_rename() {
    let source = concat!(
        "#let destination = vault::guide;\n",
        "= Nested\n#use vault::guide as nested;\n",
        "[[destination::\"Example\"]]\n\n",
        "[[nested::\"Example\"]]",
    );
    let (temp, workspace) = project(&[("README.not", source), ("guide.not", "= Example\nText")]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    let guide_uri = file_uri(&temp.path().join("docs/guide.not").canonicalize().unwrap());
    for needle in ["[[destination", "[[nested"] {
        assert_eq!(
            definition(&workspace, &uri, needle, false)["uri"],
            guide_uri
        );
        assert!(
            workspace
                .prepare_rename(
                    &uri,
                    &position(source, source.find(needle).unwrap(), false),
                    false
                )
                .is_err()
        );
    }
    assert_eq!(workspace.diagnostics(&uri, false), serde_json::json!([]));
}

#[test]
fn one_target_expression_with_multiple_module_arguments_has_no_single_definition() {
    let source = concat!(
        "#let make = (destination) => destination::\"Example\";\n",
        "#make(vault::guide)\n#make(vault::other)",
    );
    let (temp, workspace) = project(&[
        ("README.not", source),
        ("guide.not", "= Example\nFirst"),
        ("other.not", "= Example\nSecond"),
    ]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    assert!(definition(&workspace, &uri, "Example", false).is_null());
    assert_eq!(workspace.diagnostics(&uri, false), serde_json::json!([]));
}

#[test]
fn unavailable_plugin_outputs_do_not_become_missing_label_diagnostics() {
    let source = "[[vault::guide::\"Generated\"]]";
    let (temp, workspace) = project(&[
        ("README.not", source),
        ("guide.notc", "wasm \"plugin.wasm\";"),
    ]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    // A valid disk file still must not be loaded by the editor's snapshot.
    fs::write(
        temp.path().join("docs/plugin.wasm"),
        wat::parse_str("(module)").unwrap(),
    )
    .unwrap();
    assert!(definition(&workspace, &uri, "Generated", false).is_null());
    assert_eq!(workspace.diagnostics(&uri, false), serde_json::json!([]));
}

#[test]
fn standalone_documents_support_self_label_queries() {
    let uri = "file:///tmp/notist-label-editor-standalone.not";
    let source = "= Local\n[[self::\"Local\"]]\n\n[[self::\"Missing\"]]";
    let mut workspace = Workspace::default();
    workspace.open(uri.into(), source.into(), Some(1));
    assert_eq!(
        definition(&workspace, uri, "[[self::\"Local\"]]", false)["uri"],
        uri
    );
    assert!(definition(&workspace, uri, "Missing", false).is_null());
    assert_eq!(
        workspace.diagnostics(uri, false).as_array().unwrap().len(),
        1
    );
}

#[test]
fn uncalled_reserved_root_targets_navigate_but_namespaces_without_source_do_not() {
    let source = concat!(
        "#let unused = () => self::\"Local\";\n",
        "= Local\n[[vault::namespace]]",
    );
    let (temp, workspace) = project(&[
        ("README.not", source),
        ("namespace/child.not", "= Child\nText"),
    ]);
    let uri = file_uri(&temp.path().join("docs/README.not").canonicalize().unwrap());
    let target = definition(&workspace, &uri, "self::\"Local\"", false);
    assert_eq!(target["uri"], uri);
    assert_eq!(
        target["range"]["start"],
        position(source, source.find("= Local").unwrap(), false)
    );
    assert!(definition(&workspace, &uri, "[[vault::namespace]]", false).is_null());
    assert_eq!(workspace.diagnostics(&uri, false), serde_json::json!([]));
}

#[test]
fn invalid_labels_are_visible_even_when_plugin_evaluation_is_unavailable() {
    let uri = "file:///tmp/notist-invalid-label.not";
    let mut workspace = Workspace::default();
    for prefix in ["", "#wasm \"unavailable.wasm\";\n"] {
        let source = format!("{prefix}@(label: 3)\n= Heading\n[[self::\"Heading\"]]");
        workspace.open(uri.into(), source, Some(1));
        let diagnostics = workspace.diagnostics(uri, false);
        let diagnostics = diagnostics.as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0]["code"], "invalid_label");
    }
    workspace.open(uri.into(), "@!(label: 3)\n= Heading".into(), Some(2));
    assert_eq!(workspace.diagnostics(uri, false), serde_json::json!([]));
}
