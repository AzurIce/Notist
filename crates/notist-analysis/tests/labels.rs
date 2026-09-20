use notist_analysis::EvaluationSession;
use notist_eval::{ResolvedTarget, TargetError};
use notist_ir::Target;

fn session(source: &str) -> EvaluationSession {
    let mut session = EvaluationSession::default();
    session.sources.insert("README.not".into(), source.into());
    session
}

fn resolve(
    session: &mut EvaluationSession,
    labels: &[&str],
) -> Result<ResolvedTarget, TargetError> {
    session.snapshot().resolve_target(&Target::new(
        "root",
        labels.iter().map(|s| (*s).into()).collect(),
    ))
}

#[test]
fn labels_match_ordered_ancestors_and_require_the_last_segment_at_the_target() {
    let mut session = session("= 第一章\n== 安装\n=== 例子\nBody\n");
    for labels in [
        vec!["例子"],
        vec!["安装", "例子"],
        vec!["第一章", "例子"],
        vec!["第一章", "安装", "例子"],
    ] {
        let target = resolve(&mut session, &labels).unwrap().item.unwrap();
        assert_eq!(target.path, ["第一章", "安装", "例子"]);
    }
    assert_eq!(
        resolve(&mut session, &["例子", "第一章"]),
        Err(TargetError::MissingLabel)
    );
    assert_eq!(
        resolve(&mut session, &["第一章"])
            .unwrap()
            .item
            .unwrap()
            .path,
        ["第一章"]
    );
    assert!(resolve(&mut session, &[]).unwrap().item.is_none());
}

#[test]
fn repeated_labels_are_legal_and_ambiguity_counts_occurrences_not_match_combinations() {
    let mut session = session("= A\n== A\n=== leaf\n");
    assert!(session.snapshot().check("README.not").warnings.is_empty());
    assert!(resolve(&mut session, &["A", "leaf"]).is_ok());
    let Err(TargetError::Ambiguous(matches)) = resolve(&mut session, &["A"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert!(resolve(&mut session, &["A", "A"]).is_ok());
    session.sources.insert(
        "README.not".into(),
        "= A\n== leaf\n== leaf\n= B\n== leaf\n".into(),
    );
    let Err(TargetError::Ambiguous(matches)) = resolve(&mut session, &["A", "leaf"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert!(resolve(&mut session, &["B", "leaf"]).is_ok());
}

#[test]
fn explicit_labels_override_titles_and_default_labels_use_evaluated_inline_content() {
    let mut session = session(
        "#let suffix = \"内容\";\n= *动态* #text(suffix) `raw` $x$\n@(label: \"固定\")\n== 显示标题\n@(label: \"段落\")\nText\n",
    );
    assert!(resolve(&mut session, &["动态 内容 raw x"]).is_ok());
    assert!(resolve(&mut session, &["动态 内容 raw x", "固定", "段落"]).is_ok());
    assert_eq!(
        resolve(&mut session, &["显示标题"]),
        Err(TargetError::MissingLabel)
    );
    let content = session.evaluate("README.not").content;
    let html = notist_html::render(&content);
    assert!(html.contains("data-notist-label=\"固定\""));
    assert!(!html.contains(" id="));
}

#[test]
fn generated_and_repeated_items_are_distinct_even_with_the_same_source_location() {
    let mut session = session(
        "#let make = () => item(\"section\", (title: [例子], body: []));\n#let same = make();\n#same #same\n",
    );
    let Err(TargetError::Ambiguous(matches)) = resolve(&mut session, &["例子"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].location, matches[1].location);
}

#[test]
fn label_constraints_do_not_reuse_one_ancestor_twice() {
    let mut session = session("= A\n== leaf\n");
    assert_eq!(
        resolve(&mut session, &["A", "A", "leaf"]),
        Err(TargetError::MissingLabel)
    );
}

#[test]
fn invalid_labels_report_errors_without_rejecting_repeated_valid_labels() {
    for source in ["@(label: 3)\nText", "@(label: \"\")\n= Title"] {
        let result = session(source).snapshot().check("README.not");
        assert!(
            result
                .warnings
                .iter()
                .any(|s| s.contains("non-empty String")),
            "{:?}",
            result.warnings
        );
    }
    assert!(
        session("@(label: \"same\")\nText\n\n@(label: \"same\")\nText")
            .snapshot()
            .check("README.not")
            .warnings
            .is_empty()
    );
}

#[test]
fn check_reports_missing_and_ambiguous_references_at_their_sources() {
    let mut session = session(
        "[[self::\"例子\"]]\n[[self::\"missing\"]]\n[[vault::absent::\"x\"]]\n= A\n== 例子\n= B\n== 例子\n",
    );
    let snapshot = session.snapshot();
    let result = snapshot.check("README.not");
    assert_eq!(result.warnings.len(), 3, "{:?}", result.warnings);
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s.contains("ambiguous LabelPath (2 matching Items)"))
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s.contains("no matching Item"))
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s.contains("missing target module"))
    );
    assert!(result.warnings.iter().all(|s| s.starts_with("README.not:")));
}

#[test]
fn module_paths_are_canonicalized_without_evaluating_destination_and_links_do_not_cycle() {
    let mut session = session("#let unused = vault::bad::\"x\";\n[[vault::other]]\n= Home\n");
    session
        .sources
        .insert("bad.not".into(), "#error(\"destination executed\")".into());
    session
        .sources
        .insert("other.not".into(), "[[vault::\"Home\"]]".into());
    assert!(session.evaluate("README.not").warnings.is_empty());
    let snapshot = session.snapshot();
    let result = snapshot.check("README.not");
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(result.warnings[0].contains("destination executed"));
    assert!(snapshot.check("other.not").warnings.is_empty());
}

#[test]
fn reference_values_preserve_the_lexical_module_across_packages() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "p0/docs/README.notc".into(),
        "use dep::make; make();".into(),
    );
    session.sources.insert(
        "p1/docs/README.not".into(),
        "#let make = () => self::\"Local\";\n= Local\n".into(),
    );
    session
        .dependencies
        .entry("p0".into())
        .or_default()
        .insert("dep".into(), "p1".into());
    let snapshot = session.snapshot();
    let result = snapshot.check("p0/docs/README.notc");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(
        result.content.to_json()["sequence"][0]["target"]["module"],
        "p1"
    );
}
