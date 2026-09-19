use notist_analysis::{Workspace, file_uri, position};
use serde_json::Value;

fn hover(source: &str, path: &str, needle: &str, markdown: bool) -> Value {
    let mut workspace = Workspace::default();
    workspace.open(path.into(), source.into(), Some(1));
    assert!(
        workspace.documents[path].parsed.errors.is_empty(),
        "{:?}",
        workspace.documents[path].parsed.errors
    );
    workspace.hover_with_format(
        path,
        &position(source, source.rfind(needle).unwrap(), false),
        false,
        markdown,
    )
}

#[test]
fn docs_attach_to_declarations_in_both_frontends() {
    for (path, prefix) in [("file:///tmp/docs.notc", ""), ("file:///tmp/docs.not", "#")] {
        let source = format!(
            "//! Module only.\n/// *Summary* _details_.\n///\n/// - `a_b`\n/// - $x_1$\n{prefix}let value = (arg: Int = 2) => arg;\n{prefix}value;"
        );
        let rendered = hover(&source, path, "value;", true);
        let text = rendered["contents"]["value"].as_str().unwrap();
        assert!(text.contains("let value: (arg: Int = 2) -> Int"), "{text}");
        assert!(text.contains("**Summary** *details*"), "{text}");
        assert!(text.contains("- ` a_b `"), "{text}");
        assert!(!text.contains("Module only"));
        let plain = hover(&source, path, "value", false);
        let plain = plain["contents"]["value"].as_str().unwrap();
        assert!(plain.contains("Summary details."));
        assert!(plain.contains("a_b"));
        assert!(plain.contains("x_1"));
    }
}

#[test]
fn comments_do_not_leak_across_bindings_or_literals() {
    for source in [
        "/// Detached\n\nlet value = 1;\nvalue;",
        "/// Detached\n// Ordinary\nlet value = 1;\nvalue;",
        "let earlier = 0; /// Detached\nlet value = 1;\nvalue;",
        "//// Detached\nlet value = 1;\nvalue;",
        "/// Detached\nlet earlier = 0;\nlet value = earlier;\nvalue;",
        "let raw = [`\n/// Detached\n`];\nlet value = 1;\nvalue;",
    ] {
        let result = hover(source, "file:///tmp/docs.notc", "value;", true);
        assert!(
            !result["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Detached")
        );
    }
    let source = "= Section\n/// Nested binding.\n#let value = 1;\n#value";
    let result = hover(source, "file:///tmp/docs.not", "value", true);
    assert!(
        result["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Nested binding")
    );
}

#[test]
fn documentation_displays_code_as_text_and_escapes_markdown() {
    let source =
        "/// *Safe #error(123)* @!(id: 1) <script> &copy;\n/// `a_b`\nlet value = 1;\nvalue;";
    let result = hover(source, "file:///tmp/docs.notc", "value;", true);
    let text = result["contents"]["value"].as_str().unwrap();
    assert!(text.contains("\\#error\\(123\\)"), "{text}");
    assert!(text.contains("\\<script\\>"), "{text}");
    assert!(text.contains("\\&copy;"), "{text}");
    let plain = hover(source, "file:///tmp/docs.notc", "value;", false);
    let plain = plain["contents"]["value"].as_str().unwrap();
    assert!(plain.contains("#error(123)"));
    assert!(plain.contains("@!(id: 1)"));
    assert!(plain.contains("a_b"));
}

#[test]
fn imports_and_modules_read_documentation_from_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for dir in ["docs", "dep/docs"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(
        root.join("Notist.toml"),
        "[package]\nname = 'consumer'\n[dependencies.dep]\npath = 'dep'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("dep/Notist.toml"),
        "[package]\nname = 'library'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("dep/docs/README.notc"),
        "//! Library *overview*.\n/// Export description.\nlet value = 42;\n",
    )
    .unwrap();
    let source = "use dep::value as imported;\nimported;\ndep;";
    std::fs::write(root.join("docs/README.notc"), source).unwrap();
    let mut workspace = Workspace::default();
    workspace.load(root);
    let uri = file_uri(&root.join("docs/README.notc"));
    for (needle, expected) in [
        ("imported;\n", "Export description"),
        ("dep;", "Library **overview**"),
    ] {
        let result = workspace.hover_with_format(
            &uri,
            &position(source, source.rfind(needle).unwrap(), false),
            false,
            true,
        );
        assert!(
            result["contents"]["value"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{result}"
        );
    }
}
