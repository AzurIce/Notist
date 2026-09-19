use notist_analysis::{Workspace, file_uri};
use serde_json::json;
use std::{fs, path::Path, sync::Arc};

fn package(root: &Path, dependencies: &str, source: &str) {
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(
        root.join("Notist.toml"),
        format!("[package]\nname = 'example'\n{dependencies}"),
    )
    .unwrap();
    fs::write(root.join("docs/README.notc"), source).unwrap();
}

#[test]
fn discovers_nested_packages_and_shares_external_dependency_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let a = temp.path().join("a");
    let b = root.join("examples/b");
    let c = root.join("examples/c");
    package(&root, "", "let root_only = 1;");
    package(&a, "", "let value = 42;");
    package(
        &b,
        "[dependencies.shared]\npath = '../../../a'",
        "use shared::value;\nvalue;",
    );
    package(
        &c,
        "[dependencies.shared]\npath = '../../../a'",
        "use shared::*;\nvalue;",
    );
    let mut workspace = Workspace::default();
    workspace.load(&root);
    assert!(
        workspace.projects.errors.is_empty(),
        "{:?}",
        workspace.projects.errors
    );
    assert_eq!(workspace.projects.packages.len(), 4);
    assert_eq!(workspace.documents.len(), 4);
    let au = file_uri(&a.join("docs/README.notc"));
    let bu = file_uri(&b.join("docs/README.notc"));
    let cu = file_uri(&c.join("docs/README.notc"));
    let old = workspace.documents[&au].parsed.clone();
    for uri in [&bu, &cu] {
        assert_eq!(
            workspace.definition(uri, &json!({"line":1,"character":2}), false)["uri"],
            au
        );
        assert!(!workspace.completions(uri).to_string().contains("root_only"));
    }
    workspace.refresh();
    assert!(Arc::ptr_eq(&old, &workspace.documents[&au].parsed));
    workspace.open(au.clone(), "\nlet value = 7;".into(), Some(1));
    let overlay = workspace.documents[&au].parsed.clone();
    workspace.refresh();
    assert!(Arc::ptr_eq(&overlay, &workspace.documents[&au].parsed));
    assert_eq!(
        workspace.definition(&bu, &json!({"line":1,"character":2}), false)["range"]["start"]["line"],
        1
    );
    workspace.open(au.clone(), "let renamed = 7;".into(), Some(2));
    assert!(
        workspace
            .definition(&bu, &json!({"line":1,"character":2}), false)
            .is_null()
    );
    assert!(workspace.completions(&cu).to_string().contains("renamed"));
    workspace.open(au.clone(), String::new(), None);
    workspace.refresh();
    assert_eq!(workspace.documents[&au].text, "let value = 42;");
}

#[test]
fn isolates_broken_manifests_and_handles_cycles_without_evaluation() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    package(
        &a,
        "[dependencies.b]\npath = '../b'",
        "#wasm should not run",
    );
    package(&b, "[dependencies.a]\npath = '../a'", "let okay = 1;");
    fs::create_dir_all(temp.path().join("broken")).unwrap();
    fs::write(temp.path().join("broken/Notist.toml"), "not valid toml!").unwrap();
    let mut workspace = Workspace::default();
    workspace.load(temp.path());
    assert_eq!(workspace.projects.packages.len(), 3);
    assert_eq!(workspace.projects.errors.len(), 2);
    assert_eq!(workspace.documents.len(), 2);
}

#[test]
fn disk_changes_and_removed_roots_preserve_open_documents() {
    let temp = tempfile::tempdir().unwrap();
    package(temp.path(), "", "let x = 1;");
    let file = temp.path().join("docs/README.notc");
    let uri = file_uri(&file);
    let mut workspace = Workspace::default();
    workspace.load(temp.path());
    fs::write(&file, "let y = 2;").unwrap();
    workspace.refresh();
    assert_eq!(workspace.documents[&uri].text, "let y = 2;");
    workspace.open(uri.clone(), "let edit = 3;".into(), Some(4));
    fs::remove_file(&file).unwrap();
    workspace.set_roots([]);
    assert_eq!(workspace.documents[&uri].text, "let edit = 3;");
    workspace.documents.remove(&uri);
    workspace.refresh();
    assert!(workspace.documents.is_empty());
    assert!(workspace.projects.packages.is_empty());
}
