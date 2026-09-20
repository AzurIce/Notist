use notist_analysis::{EvaluationSession, Workspace, file_uri};
use notist_eval::ModuleProvider;
use std::{fs, sync::Arc};

#[test]
fn snapshots_share_unchanged_parses_and_freeze_sources_and_dependencies() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "p0/docs/README.notc".into(),
        "use dep::answer; answer;".into(),
    );
    session
        .sources
        .insert("p1/docs/README.notc".into(), "let answer = 1;".into());
    session
        .sources
        .insert("p2/docs/README.notc".into(), "let answer = 2;".into());
    session
        .dependencies
        .insert("p0".into(), [("dep".into(), "p1".into())].into());
    let before = session.snapshot();
    let unchanged = session.snapshot();
    for (path, source) in before.sources() {
        assert!(Arc::ptr_eq(
            &source.parsed,
            &unchanged.source(path).unwrap().parsed
        ));
    }
    session
        .dependencies
        .get_mut("p0")
        .unwrap()
        .insert("dep".into(), "p2".into());
    session
        .sources
        .insert("p2/docs/README.notc".into(), "let answer = 3;".into());
    session.sources.remove("p1/docs/README.notc");
    let after = session.snapshot();
    assert!(Arc::ptr_eq(
        &before.source("p0/docs/README.notc").unwrap().parsed,
        &after.source("p0/docs/README.notc").unwrap().parsed
    ));
    assert!(!Arc::ptr_eq(
        &before.source("p2/docs/README.notc").unwrap().parsed,
        &after.source("p2/docs/README.notc").unwrap().parsed
    ));
    assert!(after.source("p1/docs/README.notc").is_none());
    for (snapshot, expected) in [(&before, "1"), (&after, "3")] {
        let result = snapshot.evaluate("p0/docs/README.notc");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.content.to_json()["sequence"][0]["text"], expected);
    }
}

#[test]
fn namespace_only_modules_resolve_without_source_files() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "README.notc".into(),
        "use vault::group; group::leaf::answer;".into(),
    );
    session
        .sources
        .insert("group/leaf.notc".into(), "let answer = 42;".into());
    let snapshot = session.snapshot();
    assert_eq!(
        snapshot.module_source("root::group"),
        Some("group/README.notc")
    );
    assert!(snapshot.source("group/README.notc").is_none());
    let result = snapshot.evaluate("README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.to_json()["sequence"][0]["text"], "42");
}

#[test]
fn workspace_snapshot_reuses_overlay_parse_and_retains_origin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join("docs")).unwrap();
    fs::write(root.join("Notist.toml"), "[package]\nname = 'test'\n").unwrap();
    fs::write(
        root.join("docs/README.notc"),
        "use vault::other::answer; answer;",
    )
    .unwrap();
    fs::write(root.join("docs/other.notc"), "let answer = 1;").unwrap();
    let uri = file_uri(&root.join("docs/other.notc"));
    let mut workspace = Workspace::default();
    workspace.load(&root);
    workspace.open(uri.clone(), "let answer = 99;".into(), Some(1));
    let snapshot = workspace.snapshot();
    assert!(Arc::ptr_eq(
        &workspace.documents[&uri].parsed,
        &snapshot.source("p0/docs/other.notc").unwrap().parsed
    ));
    assert_eq!(snapshot.origin("p0/docs/other.notc"), Some(uri.as_str()));
    workspace.open(uri, "let answer = 100;".into(), Some(2));
    let result = snapshot.evaluate("p0/docs/README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.to_json()["sequence"][0]["text"], "99");
}

#[test]
fn binary_bytes_are_frozen_with_the_snapshot() {
    let mut session = EvaluationSession::default();
    session
        .binaries
        .insert("p0/wasm/lib.wasm".into(), vec![1, 2, 3]);
    let snapshot = session.snapshot();
    session.binaries.get_mut("p0/wasm/lib.wasm").unwrap()[0] = 9;
    assert_eq!(
        snapshot.binary("p0/wasm/lib.wasm"),
        Some([1, 2, 3].as_slice())
    );
    assert_eq!(
        snapshot
            .binary_path("p0/docs/README.notc", "../wasm/lib.wasm")
            .unwrap(),
        "p0/wasm/lib.wasm"
    );
    assert!(
        snapshot
            .binary_path("p0/docs/README.notc", "../../escape.wasm")
            .is_err()
    );
}
