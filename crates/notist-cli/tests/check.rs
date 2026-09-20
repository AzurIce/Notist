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
