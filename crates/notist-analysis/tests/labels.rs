use notist_analysis::{DiagnosticCode, EvaluationSession, ResolveError, ResolvedTarget, Snapshot};
use notist_ir::Target;

fn snapshot(source: &str) -> Snapshot {
    let mut session = EvaluationSession::default();
    session.sources.insert("README.not".into(), source.into());
    session.snapshot()
}
fn resolve<'s>(
    snapshot: &'s Snapshot,
    labels: &[&str],
) -> Result<ResolvedTarget<'s>, ResolveError<'s>> {
    snapshot.resolve_target(&Target::new(
        "root",
        labels.iter().map(|s| (*s).into()).collect(),
    ))
}
#[test]
fn labels_match_ordered_ancestors_and_require_the_last_segment_at_the_target() {
    let s = snapshot("= 第一章\n== 安装\n=== 例子\nBody\n");
    for labels in [
        vec!["例子"],
        vec!["安装", "例子"],
        vec!["第一章", "例子"],
        vec!["第一章", "安装", "例子"],
    ] {
        let ResolvedTarget::Item(item) = resolve(&s, &labels).unwrap() else {
            panic!()
        };
        assert_eq!(item.label_path(), ["第一章", "安装", "例子"]);
    }
    assert!(matches!(
        resolve(&s, &["例子", "第一章"]),
        Err(ResolveError::MissingLabel(_))
    ));
    assert!(matches!(resolve(&s, &[]), Ok(ResolvedTarget::Module(_))));
}
#[test]
fn repeated_labels_are_legal_and_ambiguity_counts_occurrences_not_match_combinations() {
    let s = snapshot("= A\n== A\n=== leaf\n");
    assert!(s.check("README.not").unwrap().is_ok());
    assert!(resolve(&s, &["A", "leaf"]).is_ok());
    let Err(ResolveError::Ambiguous(matches)) = resolve(&s, &["A"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert!(resolve(&s, &["A", "A"]).is_ok());
    let s = snapshot("= A\n== leaf\n== leaf\n= B\n== leaf\n");
    let Err(ResolveError::Ambiguous(matches)) = resolve(&s, &["A", "leaf"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert!(resolve(&s, &["B", "leaf"]).is_ok());
}
#[test]
fn explicit_labels_override_titles_and_default_labels_use_evaluated_inline_content() {
    let s = snapshot(
        "#let suffix = \"内容\";\n= *动态* #text(suffix) `raw` $x$\n@(label: \"固定\")\n== 显示标题\n@(label: \"段落\")\nText\n",
    );
    assert!(resolve(&s, &["动态 内容 raw x"]).is_ok());
    assert!(resolve(&s, &["动态 内容 raw x", "固定", "段落"]).is_ok());
    assert!(matches!(
        resolve(&s, &["显示标题"]),
        Err(ResolveError::MissingLabel(_))
    ));
    let html = notist_html::render(&s.evaluate("README.not").content);
    assert!(html.contains("data-notist-label=\"固定\""));
    assert!(!html.contains(" id="));
}
#[test]
fn generated_and_repeated_items_are_distinct_even_with_the_same_source_location() {
    let s = snapshot(
        "#let make = () => item(\"section\", (title: [例子], body: []));\n#let same = make();\n#same #same\n",
    );
    let Err(ResolveError::Ambiguous(matches)) = resolve(&s, &["例子"]) else {
        panic!()
    };
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].origin().location, matches[1].origin().location);
    assert_eq!(matches[0].label_path(), matches[1].label_path());
    assert_ne!(matches[0], matches[1]);
}
#[test]
fn label_constraints_do_not_reuse_one_ancestor_twice() {
    let s = snapshot("= A\n== leaf\n");
    assert!(matches!(
        resolve(&s, &["A", "A", "leaf"]),
        Err(ResolveError::MissingLabel(_))
    ));
}
#[test]
fn invalid_labels_report_errors_without_rejecting_repeated_valid_labels() {
    for source in ["@(label: 3)\nText", "@(label: \"\")\n= Title"] {
        let s = snapshot(source);
        assert!(
            s.check("README.not")
                .unwrap()
                .diagnostics()
                .iter()
                .any(|d| d.code == DiagnosticCode::InvalidLabel)
        );
        assert!(matches!(
            resolve(&s, &["missing"]),
            Err(ResolveError::IncompleteEvaluation(_))
        ));
    }
    assert!(
        snapshot("@(label: \"same\")\nText\n\n@(label: \"same\")\nText")
            .check("README.not")
            .unwrap()
            .is_ok()
    );
}
#[test]
fn check_reports_missing_and_ambiguous_references_at_their_sources() {
    let s = snapshot(
        "[[self::\"例子\"]]\n[[self::\"missing\"]]\n[[vault::absent::\"x\"]]\n= A\n== 例子\n= B\n== 例子\n",
    );
    let diagnostics = s.check("README.not").unwrap().diagnostics();
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    for code in [
        DiagnosticCode::AmbiguousLabel,
        DiagnosticCode::MissingLabel,
        DiagnosticCode::MissingModule,
    ] {
        assert!(diagnostics.iter().any(|d| d.code == code));
    }
    assert!(
        diagnostics
            .iter()
            .all(|d| d.span.as_ref().unwrap().source == "README.not")
    );
}
#[test]
fn module_paths_are_canonicalized_without_evaluating_destination_and_links_do_not_cycle() {
    let mut session = EvaluationSession::default();
    session.sources.insert(
        "README.not".into(),
        "#let unused = vault::bad::\"x\";\n[[vault::other]]\n= Home\n".into(),
    );
    session
        .sources
        .insert("bad.not".into(), "#error(\"destination executed\")".into());
    session
        .sources
        .insert("other.not".into(), "[[vault::\"Home\"]]".into());
    let s = session.snapshot();
    assert!(s.evaluate("README.not").warnings.is_empty());
    let diagnostics = s.check("README.not").unwrap().diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, DiagnosticCode::IncompleteEvaluation);
    assert!(
        diagnostics[0]
            .related
            .iter()
            .any(|d| d.message == "destination executed")
    );
    assert!(s.check("other.not").unwrap().is_ok());
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
    let s = session.snapshot();
    let result = s.check("p0/docs/README.notc").unwrap();
    assert!(result.is_ok(), "{:?}", result.diagnostics());
    assert_eq!(result.evaluation.output_links()[0].target.module, "p1");
}
