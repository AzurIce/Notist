use std::{fs, path::Path, process::Command};

fn eval(package: &Path, options: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("eval")
        .arg(package)
        .args(options)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn html_and_bundle_render_the_same_package() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../notist-next/examples/demo");
    let html = eval(&package, &["--html"]);
    assert!(html.contains("<h1>"));
    assert!(html.contains("<notist-mermaid"));
    assert!(html.contains("<notist-shader"));

    let output = tempfile::tempdir().unwrap();
    let bundle = output.path().join("bundle");
    eval(&package, &["--bundle", bundle.to_str().unwrap()]);

    let read_json = |name| -> serde_json::Value {
        serde_json::from_slice(&fs::read(bundle.join(name)).unwrap()).unwrap()
    };
    let snapshot: serde_json::Value =
        serde_json::from_str(&eval(&package, &["--snapshot"])).unwrap();
    assert_eq!(read_json("content.json"), snapshot["result"]["content"]);
    assert_eq!(
        read_json("attributes.json"),
        snapshot["result"]["attributes"]
    );

    let components = read_json("components.json");
    for name in ["mermaid", "shader"] {
        let entry = components[name].as_str().unwrap();
        let source = fs::read_to_string(bundle.join(entry)).unwrap();
        assert!(source.contains("render("), "{name}: {entry}");
    }
    assert_eq!(
        fs::read_to_string(bundle.join("renderer.js")).unwrap(),
        notist_html::RENDERER_JS
    );
    assert_eq!(
        fs::read_to_string(bundle.join("index.html")).unwrap(),
        notist_html::BUNDLE_HTML
    );
}
