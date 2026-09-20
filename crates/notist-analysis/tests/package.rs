#![cfg(feature = "filesystem")]

use notist_analysis::package;
use notist_html::RenderHtml;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn module_names_follow_binding_rules() {
    for (raw, expected) in [
        ("getting started", "getting_started"),
        ("my-tools", "my_tools"),
        ("安装指南", "安装指南"),
        ("2026 年计划", "_2026_年计划"),
        ("let", "_let"),
        ("vault", "_vault"),
        ("My__Tools", "My__Tools"),
    ] {
        let name = package::module_name(raw).unwrap();
        assert_eq!(name, expected);
        assert!(notist_syntax::valid_binding(&name));
    }
    for raw in ["", "---", "   "] {
        assert!(package::module_name(raw).is_err());
    }
}

#[test]
fn normalized_modules_support_nested_imports_and_report_collisions() {
    let f = Fixture::new();
    f.put("Notist.toml", "[package]\nname='test'");
    f.put(
        "docs/README.notc",
        "use vault::my_tools::{a, 子_模块::{aa, bb}}; a; aa; bb;",
    );
    f.put("docs/my tools/README.notc", "let a = \"a\";");
    f.put(
        "docs/my tools/子 模块.notc",
        "let aa = \"b\"; let bb = \"c\";",
    );
    let mut loaded = package::load(&f.0).unwrap();
    let result = loaded.runtime.evaluate(&loaded.entry);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.html(), "abc");
    assert!(
        loaded
            .runtime
            .sources
            .contains_key("p0/docs/my tools/子 模块.notc")
    );

    f.put("docs/my tools/子_模块.notc", "");
    let mut loaded = package::load(&f.0).unwrap();
    let result = loaded.runtime.evaluate(&loaded.entry);
    assert!(
        result.warnings.iter().any(|message| {
            message.contains("module path conflict")
                && message.contains("子 模块.notc")
                && message.contains("子_模块.notc")
        }),
        "{:?}",
        result.warnings
    );
}

#[test]
fn explicit_module_segments_are_not_normalized() {
    for source in [
        "use vault::2026 as year;",
        "use vault::my-tools;",
        "use vault::let as value;",
    ] {
        assert!(
            !notist_syntax::parse_traced(source).errors.is_empty(),
            "{source}"
        );
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "notist-package-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn put(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn transitive_resources_and_shared_dependencies() {
    let f = Fixture::new();
    f.put(
        "root/Notist.toml",
        "[package]\nname='root'\n[dependencies]\na={path='../a'}\nb={path='../b'}",
    );
    f.put("root/docs/README.notc", "use a::value; value;");
    for name in ["a", "b"] {
        f.put(
            &format!("{name}/Notist.toml"),
            &format!("[package]\nname='{name}'\n[dependencies]\nc={{path='../c'}}"),
        );
        f.put(
            &format!("{name}/docs/README.notc"),
            "use c::value as imported; let value = imported;",
        );
    }
    f.put("c/Notist.toml", "[package]\nname='c'");
    f.put(
        "c/docs/README.notc",
        "let value = item(\"sample\", (x: 3));",
    );
    f.put(
        "c/components/sample/main.js",
        "export default class extends HTMLElement {}",
    );
    f.put("c/components/style.css", "sample {color:red}");
    let mut loaded = package::load(&f.0.join("root")).unwrap();
    assert_eq!(
        loaded.components.len(),
        1,
        "resources={:?}",
        loaded.resources.keys().collect::<Vec<_>>()
    );
    assert_eq!(loaded.resources.len(), 2);
    assert!(loaded.runtime.evaluate(&loaded.entry).warnings.is_empty());
}
#[test]
fn rejects_dependency_cycles_and_component_conflicts() {
    let f = Fixture::new();
    f.put(
        "a/Notist.toml",
        "[package]\nname='a'\n[dependencies]\nb={path='../b'}",
    );
    f.put("a/docs/README.notc", "");
    f.put(
        "b/Notist.toml",
        "[package]\nname='b'\n[dependencies]\na={path='../a'}",
    );
    f.put("b/docs/README.notc", "");
    assert!(
        package::load(&f.0.join("a"))
            .err()
            .unwrap()
            .contains("cyclic")
    );
    f.put(
        "a/Notist.toml",
        "[package]\nname='a'\n[dependencies]\nb={path='../b'}",
    );
    f.put("b/Notist.toml", "[package]\nname='b'");
    f.put("a/components/sample.js", "");
    f.put("b/components/sample.js", "");
    assert!(
        package::load(&f.0.join("a"))
            .err()
            .unwrap()
            .contains("component conflict")
    );
}
#[test]
fn discovers_only_component_entries_and_preserves_assets() {
    let f = Fixture::new();
    f.put("Notist.toml", "[package]\nname='test'");
    f.put("docs/README.notc", "");
    for path in [
        "simple.js",
        "complex/main.js",
        "complex/helper.js",
        "complex/nested/main.js",
        "shared/helper.js",
        "style.css",
    ] {
        f.put(&format!("components/{path}"), "resource");
    }
    let loaded = package::load(&f.0).unwrap();
    assert_eq!(
        loaded.components,
        std::collections::BTreeMap::from([
            ("simple".into(), "p0/components/simple.js".into()),
            ("complex".into(), "p0/components/complex/main.js".into()),
        ])
    );
    assert_eq!(loaded.resources.len(), 6);
    f.put("components/simple/main.js", "");
    let error = package::load(&f.0).err().unwrap();
    assert!(error.contains("component conflict"));
    assert!(error.contains("simple.js") && error.contains("simple/main.js"));
}

#[test]
fn rejects_invalid_component_names_and_manifest_declarations() {
    for path in ["Bad Name.js", "bad_name/main.js"] {
        let f = Fixture::new();
        f.put("Notist.toml", "[package]\nname='test'");
        f.put("docs/README.notc", "");
        f.put(&format!("components/{path}"), "");
        assert!(
            package::load(&f.0)
                .err()
                .unwrap()
                .contains("invalid component name")
        );
    }
    let f = Fixture::new();
    f.put(
        "Notist.toml",
        "[package]\nname='test'\n[components.old]\nentry='components/old.js'",
    );
    assert!(package::load(&f.0).err().unwrap().contains("unknown field"));
}

#[test]
fn demo_package_is_evaluable() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo");
    let mut loaded = package::load(&root).unwrap();
    let result = loaded.runtime.evaluate(&loaded.entry);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(loaded.components.len(), 2);
    assert_eq!(loaded.runtime.used_wasm.len(), 1);
}
