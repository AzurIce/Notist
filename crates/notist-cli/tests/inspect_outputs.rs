//! Black-box shape tests for the finite CLI command surface: every command
//! runs against the same two-module fixture vault and asserts on the exact
//! line shapes agents consume. The fixture exercises all three D0006
//! annotation mount points (module `@![...]`, block `@[...]`, and both the
//! bare-id and `#tag` entry spellings) so attribute visibility stays
//! observable per command. Content access (reading files, listing
//! directories) is the host's job — notist commands are the semantic layer.

#![allow(dead_code)]

const GUIDE_NOT: &str = "\
@![status = \"draft\"]

@[wip]
= 安装

先读概述，出问题时看 #<vault::troubleshoot/日志> 。

== 故障排除

对照下述步骤排查。

= 后记

完。
";

const TROUBLESHOOT_NOT: &str = "\
@![severity = \"high\"]

@[#urgent]
= 日志

看日志输出，详见 #heading[手册]@#urgent 。
";

struct Fixture(tempfile::TempDir);

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("failed to create a temporary vault");
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("guide.not"), GUIDE_NOT).unwrap();
    std::fs::write(dir.path().join("troubleshoot.not"), TROUBLESHOOT_NOT).unwrap();
    std::fs::write(dir.path().join("assets/logo.png"), "png\n").unwrap();
    std::fs::write(dir.path().join("assets/notes.txt"), "txt\n").unwrap();
    Fixture(dir)
}

fn run(fixture: &Fixture, args: &[&str]) -> String {
    run_all(fixture, args).0
}

/// Returns `(stdout, stderr)`; check-style commands print health lines on
/// stderr, so tests asserting those need both streams.
fn run_all(fixture: &Fixture, args: &[&str]) -> (String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("--no-daemon")
        .arg("--vault")
        .arg(fixture.0.path())
        .args(args)
        .output()
        .expect("failed to spawn the notist binary");
    assert!(
        output.status.success(),
        "command {args:?} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("command output is UTF-8"),
        String::from_utf8(output.stderr).expect("command stderr is UTF-8"),
    )
}

/// Returns the first line starting with `label` (status-style key/value lines).
fn line_for(output: &str, label: &str) -> String {
    output
        .lines()
        .find(|line| line.starts_with(label))
        .unwrap_or_else(|| panic!("no `{label}` line in output:\n{output}"))
        .to_owned()
}

#[test]
fn status_reports_vault_identity_and_counts() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "status"]);
    for label in [
        "Vault", "Snapshot", "Runtime", "Sources", "Modules", "Diagnostics", "Index",
    ] {
        line_for(&output, label);
    }
    assert!(line_for(&output, "Sources").contains("2"), "{output}");
    // Virtual modules count alongside source modules (vault, assets, two
    // source modules).
    assert!(line_for(&output, "Modules").contains("4"), "{output}");
}

#[test]
fn search_lexical_ranks_candidates_with_field_and_score() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "search", "概述"]);
    assert!(output.starts_with("1 source"), "{output}");
    assert!(output.contains("vault::guide"), "{output}");
    assert!(output.contains("field="), "{output}");
    assert!(output.contains("score="), "{output}");
}

#[test]
fn search_hits_postfix_annotation_tags_through_the_tag_field() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "search", "urgent", "--fields", "tag"]);
    assert!(output.contains("vault::troubleshoot"), "{output}");
    assert!(output.contains("field=tag"), "{output}");
}

#[test]
fn items_lists_addressable_items_with_attribute_annotations() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "items", "vault::guide"]);
    assert!(output.starts_with("3 items"), "{output}");
    let installed = output
        .lines()
        .find(|line| line.starts_with("安装"))
        .expect("item line for 安装");
    assert!(installed.contains("scope L1"), "{output}");
    assert!(installed.contains("lines 4..4"), "{output}");
    assert!(installed.contains("@[wip]"), "{output}");
    assert!(output.contains("故障排除  scope L2  lines 8..8"), "{output}");
    assert!(output.contains("后记  scope L1  lines 12..12"), "{output}");
    assert!(!output.contains("ambiguous"), "{output}");
    // Identity commands carry no file paths.
    assert!(!output.contains(".not"), "{output}");
}

#[test]
fn items_lists_resource_files_of_a_module_namespace() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "items", "vault::assets"]);
    assert!(output.starts_with("2 items"), "{output}");
    assert!(
        output.contains("logo.png  resource:image  assets/logo.png"),
        "{output}"
    );
    assert!(
        output.contains("notes.txt  resource:file  assets/notes.txt"),
        "{output}"
    );
}

#[test]
fn items_claims_block_and_postfix_annotations_on_headings() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "items", "vault::troubleshoot"]);
    assert!(output.starts_with("2 items"), "{output}");
    let log = output
        .lines()
        .find(|line| line.starts_with("日志"))
        .expect("item line for 日志");
    assert!(log.contains("@[#urgent]"), "{output}");
    let manual = output
        .lines()
        .find(|line| line.starts_with("手册"))
        .expect("item line for 手册");
    assert!(manual.contains("scope L1"), "{output}");
    assert!(manual.contains("@[#urgent]"), "{output}");
}

#[test]
fn references_lists_resolved_links_with_positions() {
    let vault = fixture();
    // Scope-level targets are found through their ItemPath selector.
    let incoming = run(
        &vault,
        &["inspect", "references", "vault::troubleshoot/日志"],
    );
    assert!(incoming.starts_with("1 reference (incoming)"), "{incoming}");
    assert!(incoming.contains("vault::guide"), "{incoming}");
    assert!(incoming.contains("vault::troubleshoot"), "{incoming}");
    assert!(incoming.contains("->"), "{incoming}");
    assert!(incoming.contains("\n    "), "{incoming}");

    let outgoing = run(
        &vault,
        &["inspect", "references", "vault::guide", "--direction", "outgoing"],
    );
    assert!(outgoing.contains("1 reference (outgoing)"), "{outgoing}");
    assert!(outgoing.contains("vault::troubleshoot"), "{outgoing}");
    assert!(outgoing.contains("guide.not:"), "{outgoing}");
}

#[test]
fn definition_maps_a_byte_offset_to_the_target_identity() {
    let vault = fixture();
    // Offsets must fall inside the reference range; probe the target text.
    let offset = GUIDE_NOT.find("troubleshoot").unwrap();
    let output = run(
        &vault,
        &["inspect", "definition", "guide.not", &offset.to_string()],
    );
    assert!(output.contains("vault::troubleshoot/日志"), "{output}");
    assert!(output.contains("troubleshoot.not:"), "{output}");
    assert!(output.contains(".."), "{output}");
}

#[test]
fn ancestors_module_selector_prints_the_full_attribute_tree() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "ancestors", "vault::guide"]);
    assert!(output.contains("fingerprint"), "{output}");
    assert!(output.contains("@![status = draft]"), "{output}");
    assert!(output.contains("core::section \"安装\" L1"), "{output}");
    assert!(output.contains("core::section \"故障排除\" L2"), "{output}");
    assert!(output.contains("core::section \"后记\" L1"), "{output}");
}

#[test]
fn ancestors_item_selector_prints_the_named_scope_branch() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "ancestors", "vault::troubleshoot/日志"]);
    assert!(output.contains("@![severity = high]"), "{output}");
    assert!(output.contains("@[#urgent]"), "{output}");
    assert!(!output.contains("安装"), "{output}");
}

#[test]
fn ancestors_point_prints_the_containing_chain() {
    let vault = fixture();
    let offset = GUIDE_NOT.find("概述").unwrap();
    let output = run(
        &vault,
        &[
            "inspect",
            "ancestors",
            "guide.not",
            "--offset",
            &offset.to_string(),
        ],
    );
    assert!(output.contains("core::text"), "{output}");
    assert!(output.contains("core::paragraph"), "{output}");
    assert!(
        output.contains("core::section \"安装\" L1  lines 4..10"),
        "{output}"
    );
    assert!(output.contains("@[wip]"), "{output}");
    assert!(!output.contains("后记"), "{output}");
}

#[test]
fn ancestors_byte_range_prints_every_grazed_scope() {
    let vault = fixture();
    let start = GUIDE_NOT.find("出问题").unwrap();
    let end = GUIDE_NOT.find("完。").unwrap() + "完。".len();
    let output = run(
        &vault,
        &[
            "inspect",
            "ancestors",
            "guide.not",
            "--byte-range",
            &format!("{start}..{end}"),
        ],
    );
    assert!(output.contains("core::section \"安装\" L1"), "{output}");
    assert!(output.contains("core::section \"故障排除\" L2"), "{output}");
    assert!(output.contains("core::section \"后记\" L1"), "{output}");
}

#[test]
fn check_summary_reports_whole_vault_health() {
    let vault = fixture();
    let (stdout, stderr) = run_all(&vault, &["check", "--summary"]);
    assert!(stderr.contains("0 diagnostics in 2 sources"), "{stderr}");
    assert!(stdout.contains("checked 2 sources"), "{stdout}");
}
