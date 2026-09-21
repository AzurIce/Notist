use notist_analysis::*;
use notist_ir::{Target, Value};
use std::num::NonZeroUsize;

fn session(files: &[(&str, &str)]) -> EvaluationSession {
    let mut s = EvaluationSession::default();
    for (name, text) in files {
        s.sources.insert((*name).into(), (*text).into());
    }
    s
}
fn target(module: &str, label: &str) -> Target {
    Target::new(module, vec![label.into()])
}
fn resolved<'s>(q: &mut QuerySession<'s>, t: &Target) -> ItemRef<'s> {
    let ResolvedTarget::Item(item) = q.resolve(t).unwrap() else {
        panic!()
    };
    item
}

#[test]
fn handles_retain_output_across_queries_session_drop_and_input_changes() {
    let mut host = session(&[("README.not", "= A\n== B\nBody"), ("other.not", "= Other")]);
    let before = host.snapshot();
    let item = {
        let mut q = before.query();
        let a = resolved(&mut q, &target("root", "A"));
        let again = resolved(&mut q, &target("root", "A"));
        assert_eq!(a, again);
        let b = resolved(&mut q, &target("root", "B"));
        assert_eq!(b.parent(), Some(a.clone()));
        assert!(a.descendants().any(|i| i == b));
        q.evaluate(&"root::other".into()).unwrap();
        assert_eq!(a.value().name, "section");
        a
    };
    host.sources.insert("README.not".into(), "= Changed".into());
    let after = host.snapshot();
    assert_eq!(item.origin().syntax.unwrap().text(), "= A\n== B\nBody");
    assert!(matches!(
        after.resolve_target(&target("root", "A")),
        Err(ResolveError::MissingLabel(_))
    ));
    assert_ne!(item, resolved(&mut before.query(), &target("root", "A")));
}

#[test]
fn namespace_identity_has_no_source_and_module_resolution_does_not_evaluate() {
    let s = session(&[
        ("README.not", "= Root"),
        ("group/leaf.not", "Text"),
        ("bad.notc", "wasm \"missing.wasm\";"),
    ])
    .snapshot();
    let mut q = s.query();
    let ResolvedTarget::Module(m) = q.resolve(&Target::new("root::group", vec![])).unwrap() else {
        panic!()
    };
    assert!(m.source().is_none());
    assert!(m.text().is_none());
    assert_eq!(
        q.evaluate(m.key()).unwrap().status(),
        EvaluationStatus::Complete
    );
    assert!(matches!(
        q.resolve(&target("root::group", "X")),
        Err(ResolveError::MissingLabel(_))
    ));
    assert!(matches!(
        q.resolve(&Target::new("root::bad", vec![])),
        Ok(ResolvedTarget::Module(_))
    ));
    assert!(matches!(
        q.resolve(&target("root::bad", "X")),
        Err(ResolveError::IncompleteEvaluation(_))
    ));
}

#[test]
fn explicit_addresses_share_boundaries_but_do_not_consult_lexical_bindings() {
    let mut host = session(&[
        ("p0/docs/README.not", "= Root"),
        ("p0/docs/a/b.not", "Text"),
        ("p1/docs/README.not", "= Dependency"),
    ]);
    host.dependencies
        .entry("p0".into())
        .or_default()
        .insert("dep".into(), "p1".into());
    let s = host.snapshot();
    let from = ModuleKey::from("p0::a::b");
    for (root, segments, expected) in [
        (ModuleRoot::Vault, vec![], "p0"),
        (ModuleRoot::Current, vec![], "p0::a::b"),
        (
            ModuleRoot::Parent(NonZeroUsize::new(1).unwrap()),
            vec![],
            "p0::a",
        ),
        (ModuleRoot::Dependency("dep".into()), vec![], "p1"),
    ] {
        assert_eq!(
            s.locate_module(&from, &ModuleAddress { root, segments })
                .unwrap()
                .key()
                .0,
            expected
        );
    }
    assert_eq!(
        s.locate_module(
            &from,
            &ModuleAddress {
                root: ModuleRoot::Parent(NonZeroUsize::new(3).unwrap()),
                segments: vec![]
            }
        )
        .unwrap_err(),
        ModuleAddressError::EscapesPackageRoot
    );
    assert!(matches!(
        s.locate_module(
            &from,
            &ModuleAddress {
                root: ModuleRoot::Vault,
                segments: vec!["bad/path".into()]
            }
        ),
        Err(ModuleAddressError::InvalidSegment(_))
    ));
}

#[test]
fn observations_keep_attempts_errors_and_unexecuted_sites_separate() {
    let text = "#let make = (destination) => destination::\"X\";\n#let unused = () => self::\"missing\";\n#make(vault::a) #make(vault::b) #make(vault::a) #make(3)";
    let s = session(&[("README.not", text), ("a.not", "= X"), ("b.not", "= X")]).snapshot();
    let exprs = s.source("README.not").unwrap().parsed.expressions();
    let target_exprs: Vec<_> = exprs
        .iter()
        .filter(|e| matches!(e.kind, notist_syntax::ExprKind::Target(..)))
        .collect();
    let mut q = s.query();
    let e = q.evaluate(&"root".into()).unwrap();
    let attempts = e
        .observations(&s.syntax("README.not", target_exprs[0].id).unwrap())
        .unwrap();
    assert_eq!(attempts.len(), 4);
    assert_eq!(attempts[0].result.as_ref().unwrap().module, "root::a");
    assert_eq!(attempts[1].result.as_ref().unwrap().module, "root::b");
    assert_eq!(attempts[0].result, attempts[2].result);
    assert!(matches!(
        attempts[3].result,
        Err(ModuleAddressError::NotModule(_))
    ));
    assert!(
        e.observations(&s.syntax("README.not", target_exprs[1].id).unwrap())
            .unwrap()
            .is_empty()
    );
    let other = s.clone();
    assert_eq!(
        e.observations(&other.syntax("README.not", target_exprs[0].id).unwrap())
            .unwrap_err(),
        ObservationError::ForeignSnapshot
    );
}

#[test]
fn ordinary_binding_shadows_dependency_and_non_module_binding_is_an_error() {
    let mut host = session(&[
        ("p0/docs/README.notc", "let dep = vault::local; dep::\"X\";"),
        ("p0/docs/local.not", "= X"),
        ("p1/docs/README.not", "= X"),
    ]);
    host.dependencies
        .entry("p0".into())
        .or_default()
        .insert("dep".into(), "p1".into());
    let s = host.snapshot();
    let e = s.query().evaluate(&"p0".into()).unwrap();
    assert_eq!(
        e.references()[0].result.as_ref().unwrap().module,
        "p0::local"
    );
    host.sources.insert(
        "p0/docs/README.notc".into(),
        "let dep = 3; dep::\"X\";".into(),
    );
    let s = host.snapshot();
    let e = s.query().evaluate(&"p0".into()).unwrap();
    assert!(matches!(
        e.references()[0].result,
        Err(ModuleAddressError::NotModule(_))
    ));
}

#[test]
fn origins_distinguish_output_owner_creation_and_forwarding() {
    let s = session(&[
        (
            "README.notc",
            "use vault::helper::make; let same = make(); same; same;",
        ),
        (
            "helper.notc",
            "let make = () => item(\"section\", (title: [X], body: [Body]));",
        ),
    ])
    .snapshot();
    let e = s.query().evaluate(&"root".into()).unwrap();
    let items: Vec<_> = e.root_items().collect();
    assert_eq!(items.len(), 2);
    assert_ne!(items[0], items[1]);
    for i in &items {
        assert_eq!(i.evaluation().module().0, "root");
        let origin = i.origin();
        assert_eq!(origin.kind, Some(OriginKind::Constructor));
        assert_eq!(origin.location.source, "helper.notc");
        assert!(origin.syntax.unwrap().text().starts_with("item("));
    }
}

#[test]
fn tree_traverses_unlabeled_nested_fields_but_not_attributes_or_links() {
    let s = session(&[(
        "README.notc",
        "let child = item(\"child\", (:)); item(\"parent\", (nested: (hidden: (child, child))));",
    )])
    .snapshot();
    let e = s.query().evaluate(&"root".into()).unwrap();
    let root = e.root_items().next().unwrap();
    assert_eq!(root.children().count(), 2);
    assert_eq!(e.items().count(), 3);
    assert!(root.label_path().is_empty());
    assert!(matches!(&root.value().args["nested"], Value::Dict(_)));
    let s = session(&[(
        "README.not",
        "@(meta: item(\"hidden\", (:)))\n= A\n[[self::\"A\"]]",
    )])
    .snapshot();
    let e = s.query().evaluate(&"root".into()).unwrap();
    assert!(e.items().all(|i| i.value().name != "hidden"));
    assert!(e.items().count() < 10);
}

#[test]
fn recovered_target_formation_failure_stays_observable_without_failing_check() {
    let s = session(&[("README.notc", "recover(super::\"X\", [Recovered]);")]).snapshot();
    let report = s.check("README.notc").unwrap();
    assert!(report.is_ok(), "{:?}", report.diagnostics());
    assert!(report.evaluation.references()[0].result.is_err());
}

#[test]
fn broken_outgoing_links_do_not_make_an_existing_item_unresolvable() {
    let s = session(&[
        ("README.not", "[[vault::other::\"X\"]]"),
        ("other.not", "= X\n[[vault::absent::\"Y\"]]"),
    ])
    .snapshot();
    let mut q = s.query();
    assert!(q.check(&"root".into()).unwrap().is_ok());
    assert!(!q.check(&"root::other".into()).unwrap().is_ok());
    assert!(q.resolve(&target("root::other", "X")).is_ok());
}

#[test]
fn module_attributes_errors_prevent_false_absence_and_query_order_is_irrelevant() {
    let s = session(&[
        (
            "README.not",
            "@!(problem: error(\"metadata failed\"))\n= Existing",
        ),
        ("other.not", "= Other"),
    ])
    .snapshot();
    let mut a = s.query();
    let mut b = s.query();
    a.evaluate(&"root::other".into()).unwrap();
    let ea = a.evaluate(&"root".into()).unwrap();
    let eb = b.evaluate(&"root".into()).unwrap();
    assert_eq!(ea.diagnostics(), eb.diagnostics());
    assert_eq!(ea.status(), EvaluationStatus::Incomplete);
    assert!(matches!(
        a.resolve(&target("root", "Missing")),
        Err(ResolveError::IncompleteEvaluation(_))
    ));
}

#[test]
fn section_source_range_and_selection_are_exact_in_frozen_unicode_source() {
    let text = "@(label: \"stable\")\n= 标题😀\n正文\n== 子节\n更多\n= Next\n";
    let s = session(&[("README.not", text)]).snapshot();
    let item = resolved(&mut s.query(), &target("root", "stable"));
    let syntax = item.origin().syntax.unwrap();
    assert_eq!(syntax.text(), "= 标题😀\n正文\n== 子节\n更多\n");
    let selection = syntax.selection_span();
    assert_eq!(&text[selection.start..selection.end], "= 标题😀");
}

#[test]
fn failed_imports_prevent_false_absence_even_when_an_export_was_available() {
    let s = session(&[
        ("README.notc", "use vault::bad::value; value;"),
        ("bad.notc", "let value = [Known]; error(\"import failed\");"),
    ])
    .snapshot();
    assert!(matches!(
        s.query().resolve(&target("root", "Missing")),
        Err(ResolveError::IncompleteEvaluation(_))
    ));
}

#[test]
fn imported_module_metadata_errors_also_make_the_entry_incomplete() {
    let s = session(&[
        ("README.notc", "use vault::bad::value; value;"),
        (
            "bad.not",
            "@!(problem: error(\"metadata failed\"))\n#let value = [Known];",
        ),
    ])
    .snapshot();
    let e = s.query().evaluate(&"root".into()).unwrap();
    assert_eq!(e.status(), EvaluationStatus::Incomplete);
    assert!(
        e.diagnostics()
            .iter()
            .any(|d| d.message.contains("metadata failed"))
    );
}

#[test]
fn repeated_output_links_are_not_erased_by_an_identical_target_value() {
    let s = session(&[("README.not", "#let link = self::\"Missing\";\n#link #link")]).snapshot();
    let report = s.check("README.not").unwrap();
    assert_eq!(report.evaluation.references().len(), 1);
    assert_eq!(report.evaluation.output_links().len(), 2);
    assert_eq!(report.references.len(), 3);
    assert_eq!(report.diagnostics().len(), 3);
}

#[test]
fn recovered_imports_do_not_poison_entry_queries_or_hide_dependency_failures() {
    for bad in [
        "#let value = [Known];\n#error(\"failed\")",
        "@!(problem: error(\"failed\"))\n#let value = [Known];",
    ] {
        for expression in ["vault::bad::value", "vault::bad"] {
            let source = format!(
                "recover({expression}, item(\"section\", (title: [Fallback], body: [OK])));"
            );
            let s = session(&[("README.notc", &source), ("bad.not", bad)]).snapshot();
            let mut q = s.query();
            assert!(q.check(&"root".into()).unwrap().is_ok());
            assert!(q.resolve(&target("root", "Fallback")).is_ok());
            assert!(!q.check(&"root::bad".into()).unwrap().is_ok());
            assert!(q.check(&"root".into()).unwrap().is_ok());
        }
    }
}

#[test]
fn module_metadata_has_no_item_label_constraint_but_nested_errors_are_checked() {
    let s = session(&[("README.not", "@!(label: 3)\n= Existing")]).snapshot();
    let mut q = s.query();
    assert!(q.check(&"root".into()).unwrap().is_ok());
    assert!(q.resolve(&target("root", "Existing")).is_ok());
    for text in [
        "@!(data: (nested: error(\"failed\")))\n= Existing",
        "@!(data: with_attributes(item(\"custom\", (:)), (label: 3)))\n= Existing",
    ] {
        let s = session(&[("README.not", text)]).snapshot();
        assert!(!s.check("README.not").unwrap().is_ok());
    }
}

#[test]
fn code_attributes_preserve_values_and_origins_and_share_markup_override_rules() {
    let s = session(&[(
        "README.notc",
        r#"
        let original = item("custom", (body: [Body]));
        let first = with_attributes(original, (label: "old", priority: 1));
        let changed = with_attributes(first, (label: "new"));
        original; first; changed;
        [@(label: "markup") #with_attributes(first, (:))];
    "#,
    )])
    .snapshot();
    let mut q = s.query();
    let e = q.evaluate(&"root".into()).unwrap();
    assert_eq!(e.status(), EvaluationStatus::Complete);
    let items: Vec<_> = e.items().filter(|i| i.value().name == "custom").collect();
    assert_eq!(items.len(), 4);
    assert!(items[0].value().attributes.is_empty());
    for (i, label) in items[1..].iter().zip(["old", "new", "markup"]) {
        assert_eq!(i.value().attributes["label"].to_json(), label);
        assert_eq!(i.value().attributes["priority"].to_json(), 1);
        assert_eq!(
            i.value().args["body"].to_json(),
            items[0].value().args["body"].to_json()
        );
        assert_eq!(
            i.origin().syntax.unwrap().node_id(),
            items[0].origin().syntax.unwrap().node_id()
        );
        assert!(q.resolve(&target("root", label)).is_ok());
    }
    for source in [
        "with_attributes(3, (:));",
        "with_attributes(item(\"custom\", (:)), 3);",
        "with_attributes(item(\"custom\", (:)), (label: 3));",
    ] {
        let s = session(&[("README.notc", source)]).snapshot();
        assert!(!s.check("README.notc").unwrap().is_ok());
    }
}
