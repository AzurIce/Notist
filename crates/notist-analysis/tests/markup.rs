use notist_analysis::EvaluationSession;
use notist_eval::Evaluation;
use notist_html::RenderHtml;
use serde_json::{Value, json};

fn run(text: &str) -> Evaluation {
    let mut runtime = EvaluationSession::default();
    runtime.sources.insert("README.not".into(), text.into());
    runtime.evaluate("README.not")
}
fn checked(text: &str) -> Value {
    let result = run(text);
    assert!(result.warnings.is_empty(), "{:?}\n{text}", result.warnings);
    result.content.to_json()
}
fn items<'a>(value: &'a Value, name: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    fn visit<'a>(v: &'a Value, name: &str, out: &mut Vec<&'a Value>) {
        if v["item"] == name {
            out.push(v);
        }
        if let Some(o) = v.as_object() {
            for v in o.values() {
                visit(v, name, out);
            }
        }
        if let Some(a) = v.as_array() {
            for v in a {
                visit(v, name, out);
            }
        }
    }
    visit(value, name, &mut out);
    out
}

#[test]
fn raw_text_is_opaque_and_fences_are_not_markdown() {
    let content = checked(
        "`#error(\"no\") [text] * _ $ @! //`\n\n```rust let x = 1;```\n\n````not\n  ```\n  @! #bad\n````\n\n``",
    );
    let raw = items(&content, "raw");
    assert_eq!(raw.len(), 4);
    assert_eq!(
        raw[0]["args"]["content"],
        "#error(\"no\") [text] * _ $ @! //"
    );
    assert_eq!(raw[1]["args"]["lang"], "rust");
    assert_eq!(raw[1]["args"]["block"], false);
    assert_eq!(raw[2]["args"]["content"], "  ```\n  @! #bad");
    assert_eq!(raw[2]["args"]["block"], true);
    assert_eq!(raw[3]["args"]["content"], "");
    assert!(!run("```rust\nmissing").warnings.is_empty());
}

#[test]
fn attributes_merge_without_inheritance_or_args_mutation() {
    let result = run(
        "#let data = (label: \"intro\", priority: 1);\n@! (title: \"one\") @data @!(title: \"two\") @(priority: 2)\n= Heading\n@!(ai: true)\nBody\n\n@(label: \"next\")\nSecond paragraph",
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.attributes["title"].to_json(), "two");
    assert_eq!(result.attributes["ai"].to_json(), true);
    let content = result.content.to_json();
    let section = items(&content, "section")[0];
    assert_eq!(section["attributes"], json!({"label":"intro","priority":2}));
    assert!(section["args"].get("label").is_none());
    let paragraphs = items(&content, "paragraph");
    assert_eq!(paragraphs[0]["attributes"], json!({}));
    assert_eq!(paragraphs[1]["attributes"]["label"], "next");
    for source in ["@1\nHello", "@!\"bad\"", "@(label: \"orphan\")"] {
        assert!(!run(source).warnings.is_empty(), "{source}");
    }
}

#[test]
fn whitespace_comments_escapes_and_word_boundaries() {
    let result = run(
        "one  two\nthree\n\nfour\\\nfive\n\nfoo_bar *strong* _emphasis_ \\* \\u{41}\n// hidden\n/* nested /* hidden */ comment */ tail",
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let html = result.content.html();
    assert!(html.contains("<p>one two three</p>"), "{html}");
    assert!(html.contains("<br>"), "{html}");
    assert!(
        html.contains("foo_bar <strong>strong</strong> <em>emphasis</em> * A"),
        "{html}"
    );
    assert!(!html.contains("hidden"));
    assert!(!run("/* unclosed").warnings.is_empty());
    assert!(!run("\\u{110000}").warnings.is_empty());
}

#[test]
fn brackets_are_text_while_code_keeps_content_boundaries() {
    let result = run("literal [brackets] [nested [brackets]] [[self]]");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(
        result
            .content
            .html()
            .contains("literal [brackets] [nested [brackets]]")
    );
    let mut runtime = EvaluationSession::default();
    runtime
        .sources
        .insert("README.notc".into(), "[literal [brackets]];".into());
    assert_eq!(
        runtime.evaluate("README.notc").content.html(),
        "<p>literal [brackets]</p>"
    );
}

#[test]
fn math_keeps_source_and_display_mode_without_evaluation() {
    let content = checked("Inline $x_1 + #unknown$ and $\\$x$\n\n$ x^2 / 2 $\n\n#math(\"a_b\")");
    let math = items(&content, "math");
    assert_eq!(math.len(), 4);
    assert_eq!(math[0]["args"]["content"], "x_1 + #unknown");
    assert_eq!(math[0]["args"]["block"], false);
    assert_eq!(math[1]["args"]["content"], "\\$x");
    assert_eq!(math[2]["args"]["block"], true);
    assert_eq!(math[3]["args"]["content"], "a_b");
    assert!(!run("$unfinished").warnings.is_empty());
    assert!(!run("#math(3)").warnings.is_empty());
    let quoted = checked("$\"$\" + x$");
    assert_eq!(items(&quoted, "math")[0]["args"]["content"], "\"$\" + x");
}

#[test]
fn lists_nest_and_preserve_numbering_and_terms() {
    let content = checked(
        "- First\n  continuation\n  + Nested\n  + Again\n- Second\n\n5. Five\n+ Six\n\n/ Term: definition\n  continuation\n/ Next: description\n\nOutside",
    );
    assert!(items(&content, "list").is_empty());
    let lists = items(&content, "list-item");
    assert_eq!(lists.len(), 6, "{content}");
    assert_eq!(lists[0]["args"]["ordered"], false);
    assert_eq!(lists[1]["args"]["ordered"], true);
    assert!(
        items(&content, "list-item")
            .iter()
            .any(|v| v["args"]["number"] == 5)
    );
    assert_eq!(items(&content, "term-item").len(), 2);
    assert!(items(&content, "terms").is_empty());
}

#[test]
fn automatic_links_and_explicit_links_escape_output() {
    let result = run(
        "https://example.com/path.\n\n#link(\"https://example.com\")[*Example*]\n\n~ ... -- --- -? -12",
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let html = result.content.html();
    assert!(html.contains("href=\"https://example.com/path\""), "{html}");
    assert!(html.contains("<strong>Example</strong>"), "{html}");
    assert!(html.contains('\u{a0}'));
    assert!(html.contains('\u{2212}'));
    assert!(
        run("#link(\"javascript:alert(1)\")[bad]")
            .content
            .html()
            .contains("notist-error")
    );
}

#[test]
fn annotations_and_interpolation_keep_node_boundaries() {
    let result = run(
        "@(label: \"custom\")\n#item(\"paragraph\", (body: [Body]))\n\n_foo_bar_\n\n#math(content: \"x\")",
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let content = result.content.to_json();
    assert_eq!(items(&content, "paragraph").len(), 3);
    assert_eq!(
        items(&content, "paragraph")[0]["attributes"]["label"],
        "custom"
    );
    assert!(result.content.html().contains("<em>foo_bar</em>"));
    assert_eq!(items(&content, "math")[0]["args"]["content"], "x");
    assert!(
        !run("@!(bad: error(\"bad module attribute\"))")
            .warnings
            .is_empty()
    );
    assert!(
        !run("@(bad: error(\"bad item attribute\"))\nText")
            .warnings
            .is_empty()
    );
    assert!(!run("/ Missing separator\nOther: text").warnings.is_empty());
}
