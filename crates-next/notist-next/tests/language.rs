use notist_next::{Content, Runtime, runtime::Value};
fn run(source: &str) -> notist_next::Evaluation {
    let mut r = Runtime::default();
    r.sources.insert("README.notc".into(), source.into());
    r.evaluate("README.notc")
}
#[test]
fn typed_defaults_named_trailing_recursion_and_capture() {
    let result = run(r#"
      let prefix = "p";
      let panel = (title: String = prefix, body: Content? = none) => item("panel", (title: title, body: body));
      let fact = (n: Int) => if n == 0 { 1 } else { n * fact(n - 1) };
      let add = (a: Int, b: Int = a + 1) => a + b;
      panel(title: "x")[#fact(5) #add(2)];
    "#);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let json = result.content.to_json();
    assert_eq!(json["sequence"][0]["item"], "panel");
    assert!(json.to_string().contains("120"));
    assert!(json.to_string().contains('5'));
}
#[test]
fn function_argument_contracts() {
    for call in [
        "f(1)",
        "f(x: \"a\", x: \"b\")",
        "f(z: \"a\")",
        "f()",
        "f(\"a\",\"b\")",
    ] {
        let result = run(&format!("let f = (x: String) => x; {call};"));
        assert!(!result.warnings.is_empty(), "{call}");
    }
    assert!(
        !run("let f = (x: String = 3) => x; f();")
            .warnings
            .is_empty()
    );
}
#[test]
fn collections_none_and_item_values() {
    let r = run(
        r#"let value = item("x", (body: [hello], data: (1, 2), empty: none)); value.args.body; value.name; none; [];"#,
    );
    assert!(r.warnings.is_empty());
    assert!(r.content.html().contains("hellox"));
    let r = run("let f = (x: String? = none) => if x == none { [empty] } else { text(x) }; f();");
    assert!(r.warnings.is_empty());
    assert_eq!(r.content.html(), "empty");
}

#[test]
fn not_markup_and_item_targets_share_the_code_pipeline() {
    let mut r = Runtime::default();
    r.sources.insert(
        "README.notc".into(),
        "use vault::guide; guide::\"intro\";".into(),
    );
    r.sources.insert(
        "guide.not".into(),
        "Hello [[vault::guide#intro]] #text(\"!\")".into(),
    );
    let result = r.evaluate("guide.not");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.content.html(),
        "Hello <a href=\"vault::guide#intro\">vault::guide#intro</a> !"
    );

    let result = r.evaluate("README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.content.html(),
        "<a href=\"guide#intro\">guide#intro</a>"
    );
}
#[test]
fn no_legacy_syntax_or_overloads() {
    for source in [
        "fn f(x: Int) = x;",
        "rule f() = [];",
        "element p() = \"notist-p\";",
        "extern fn f() -> String = \"x#f\";",
        "import x from \"x.notc\";",
        "use wasm \"x.wasm\";",
        "let f = |x| x;",
        "let x = 1; let x = 2;",
    ] {
        assert!(!run(source).warnings.is_empty(), "{source}");
    }
}
#[test]
fn paths_aliases_globs_self_super_numbers_and_spaces() {
    let mut r = Runtime::default();
    r.sources.insert(
        "p0/docs/README.notc".into(),
        r#"
      use vault::getting_started::{self as guide, title};
      use vault::_2026::_09 as month;
      use vault::helpers::*;
      title; guide::title; month::value; helper;
    "#
        .into(),
    );
    r.sources.insert(
        "p0/docs/getting started.notc".into(),
        "let title = \"guide\";".into(),
    );
    r.sources.insert(
        "p0/docs/2026/09.notc".into(),
        "use super::shared::value; let value_copy = value; let value = \"bad\";".into(),
    );
    // Imports are not re-exported and cannot be shadowed by let.
    r.sources.insert(
        "p0/docs/2026/shared.notc".into(),
        "let value = \"date\";".into(),
    );
    r.sources.insert(
        "p0/docs/helpers.notc".into(),
        "let helper = \"helper\";".into(),
    );
    assert!(!r.evaluate("p0/docs/README.notc").warnings.is_empty());
    r.sources.insert(
        "p0/docs/2026/09.notc".into(),
        "use super::shared::value as shared; let value = shared;".into(),
    );
    let result = r.evaluate("p0/docs/README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.html(), "guideguidedatehelper");
}
#[test]
fn conflicts_cycles_and_package_relative_roots() {
    let mut r = Runtime::default();
    r.sources
        .insert("README.notc".into(), "use vault::a;".into());
    r.sources.insert("a.notc".into(), "use vault;".into());
    assert!(!r.evaluate("README.notc").warnings.is_empty());
    r.sources.insert("getting started.notc".into(), "".into());
    r.sources.insert("getting_started.not".into(), "".into());
    let result = r.evaluate("README.notc");
    assert!(
        result
            .warnings
            .iter()
            .any(|e| e.contains("module path conflict"))
    );
    let mut r = Runtime::default();
    r.sources.insert(
        "p0/docs/README.notc".into(),
        "use dep::answer; answer;".into(),
    );
    r.sources.insert(
        "p1/docs/README.notc".into(),
        "use vault::local::value; let answer = value;".into(),
    );
    r.sources.insert(
        "p1/docs/local.notc".into(),
        "let value = \"own root\";".into(),
    );
    r.dependencies
        .entry("p0".into())
        .or_default()
        .insert("dep".into(), "p1".into());
    assert_eq!(r.evaluate("p0/docs/README.notc").content.html(), "own root");
}
#[test]
fn item_html_and_error_boundaries() {
    let r = run(
        r#"item("paragraph", (body: [a *b*])); item("mermaid", (source: "<script>\"", count: 3)); error("bad"); [tail];"#,
    );
    let html = r.content.html();
    assert!(html.contains("<p>a <strong>b</strong></p>"));
    assert!(html.contains("data-count=\"3\""));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.ends_with("tail"));
    assert_eq!(r.warnings.len(), 1);
    assert!(run("item(\"arbitrary.name\", (:));").warnings.is_empty());
}
#[test]
fn registration_named_defaults_and_nested_paths() {
    let registry=serde_json::json!({"functions":{"echo":{"export":"echo","params":[{"name":"source","type":"String","default":"default"}],"result":"String"}}}).to_string();
    let escaped = registry
        .bytes()
        .map(|b| format!("\\{b:02x}"))
        .collect::<String>();
    let wat = format!(
        r#"(module (memory (export "memory") 1)
       (data (i32.const 0) "{escaped}")
       (func (export "alloc") (param i32) (result i32) i32.const 8192)
       (func (export "notist_register") (param i32 i32) (result i64) i64.const {})
       (func (export "echo") (param i32 i32) (result i64)
         local.get 0 i32.const 1 i32.add i64.extend_i32_u i64.const 32 i64.shl
         local.get 1 i32.const 2 i32.sub i64.extend_i32_u i64.or))"#,
        registry.len()
    );
    let mut r = Runtime::default();
    r.sources.insert(
        "p0/docs/README.notc".into(),
        "use vault::nested::api::echo; echo(source: \"hello\"); echo();".into(),
    );
    r.sources.insert(
        "p0/docs/nested/api.notc".into(),
        "wasm \"../../wasm/semantic.wasm\";".into(),
    );
    r.binaries
        .insert("p0/wasm/semantic.wasm".into(), wat::parse_str(wat).unwrap());
    let result = r.evaluate("p0/docs/README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.html(), "hellodefault");
    assert_eq!(r.used_wasm.len(), 1);
}
#[test]
fn bounded_recursion_and_data() {
    assert!(!run("let f = () => f(); f();").warnings.is_empty());
    assert!(!Value::List(vec![Value::Named("f".into())]).serializable());
    let r = run("let f = () => error(\"bad\"); item(\"x\", (nested: (inner: f())));");
    assert_eq!(r.warnings.len(), 1);
    assert!(matches!(r.content, Content::Sequence(_)));
}
