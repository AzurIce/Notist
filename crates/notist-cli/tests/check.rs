use std::{fs, process::Command};

#[test]
fn check_accepts_repeated_labels_but_rejects_unresolved_references() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("docs")).unwrap();
    fs::write(
        dir.path().join("Notist.toml"),
        "[package]\nname = 'labels'\n",
    )
    .unwrap();
    let tree = "= A\n== 例子\nOne\n= B\n== 例子\nTwo\n";
    for (reference, success, message) in [
        ("", true, ""),
        ("[[self::\"A\"::\"例子\"]]\n", true, ""),
        ("[[self::\"例子\"]]\n", false, "ambiguous LabelPath"),
        ("[[self::\"missing\"]]\n", false, "no matching Item"),
    ] {
        fs::write(
            dir.path().join("docs/README.not"),
            format!("{reference}{tree}"),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_notist"))
            .arg("check")
            .arg(dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), success, "{stderr}");
        assert!(stderr.contains(message), "{stderr}");
    }
}

fn fixture(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("docs")).unwrap();
    fs::write(
        dir.path().join("Notist.toml"),
        "[package]\nname = 'check'\n",
    )
    .unwrap();
    fs::write(dir.path().join("docs/README.not"), source).unwrap();
    dir
}

#[test]
fn rich_diagnostics_show_source_and_each_ambiguous_candidate_without_color_in_pipes() {
    let dir = fixture("[[self::\"例子\"]]\n= A\n== 例子\nOne\n= B\n== 例子\nTwo\n");
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("check")
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    for needle in [
        "error[ambiguous_label]",
        "README.not:1:1",
        "[[self::\"例子\"]]",
        "candidate 1: A → 例子",
        "candidate 2: B → 例子",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in {text}");
    }
    assert!(!text.contains('\u{1b}'));
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .args(["check", "--color", "always"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(output.stderr.contains(&0x1b));
}

#[test]
fn json_reports_distinguish_missing_targets_from_incomplete_evaluation() {
    let dir = fixture("中文😀 [[vault::guide::\"Absent\"]]\n[[self::\"Missing\"]]\n");
    fs::write(dir.path().join("docs/guide.notc"), "wasm \"missing.wasm\";").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .args(["check", "--json", "--color", "always"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(report["coverage"], "executed_references");
    assert_eq!(report["checked_modules"], 2);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    let incomplete = diagnostics
        .iter()
        .find(|d| d["code"] == "incomplete_evaluation")
        .unwrap();
    assert_eq!(incomplete["span"]["start"], "中文😀 ".len());
    assert!(!incomplete["related"].as_array().unwrap().is_empty());
    assert!(diagnostics.iter().any(|d| d["code"] == "missing_label"));
}

#[test]
fn success_is_quiet_and_setup_failures_use_the_selected_report_format() {
    let dir = fixture("= Good\n[[self::\"Good\"]]");
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("check")
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .args(["check", "--json"])
        .arg(dir.path().join("missing"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["code"], "setup");
}

#[test]
fn syntax_and_import_errors_point_to_their_own_source_lines() {
    let dir = fixture("Text");
    fs::write(
        dir.path().join("docs/broken.notc"),
        "let value = 1;\nuse vault::absent;\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .args(["check", "--color", "never"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("broken.notc:2:1"), "{text}");
    assert!(text.contains("use vault::absent;"), "{text}");
}

#[test]
fn invalid_module_filenames_produce_setup_diagnostics_without_panicking() {
    let dir = fixture("= Root");
    fs::write(dir.path().join("docs/---.not"), "Text").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .args(["check", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["code"], "setup");
}
