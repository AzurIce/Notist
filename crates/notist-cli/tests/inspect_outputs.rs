//! Black-box shape tests for the finite CLI command surface: every command
//! runs against the same two-module fixture vault and asserts on the exact
//! line shapes agents consume. The fixture exercises all three D0006
//! annotation mount points (module `@![...]`, block `@[...]`, and both the
//! bare-id and `#tag` entry spellings) so attribute visibility stays
//! observable per command. Content access (reading files, listing
//! directories) is the host's job — notist commands are the semantic layer.

#![allow(dead_code)]

const GUIDE_NOT: &str = "\
@!(status: \"draft\")

@(wip: true)
= 安装

先读概述，出问题时看 #<vault::troubleshoot/日志> 。

== 故障排除

对照下述步骤排查。

= 后记

完。
";

const TROUBLESHOOT_NOT: &str = "\
@!(severity: \"high\")

@(tag: \"urgent\")
= 日志

看日志输出，详见 @(tag: \"urgent\")#heading[手册] 。
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
        "Vault",
        "Snapshot",
        "Runtime",
        "Sources",
        "Modules",
        "Diagnostics",
        "Index",
    ] {
        line_for(&output, label);
    }
    assert!(line_for(&output, "Sources").contains("2"), "{output}");
    // Virtual modules count alongside source modules (vault, assets, two
    // source modules).
    assert!(line_for(&output, "Modules").contains("4"), "{output}");
}

#[test]
fn ls_lists_child_modules_identity_only() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "ls"]);
    assert!(output.starts_with("3 modules"), "{output}");
    assert!(output.contains("vault::guide — 安装"), "{output}");
    assert!(output.contains("vault::assets — <virtual>"), "{output}");
    assert!(!output.contains(".not"), "{output}");
    assert!(
        !output.lines().any(|line| line.starts_with("vault ")),
        "{output}"
    );
}

#[test]
fn ls_leaf_module_is_empty() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "ls", "vault::guide"]);
    assert!(output.starts_with("0 modules"), "{output}");
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
fn search_hits_annotation_keys_and_values_through_the_tag_field() {
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
    // The scope span starts at the declaring annotation and ends at the
    // heading line: the annotation belongs to the block it declares, the
    // blank line before it belongs to no node.
    assert!(installed.contains("lines 3..4"), "{output}");
    assert!(installed.contains("@(wip: true)"), "{output}");
    assert!(
        output.contains("安装/故障排除 scope L2 lines 8..8"),
        "{output}"
    );
    assert!(output.contains("后记 scope L1 lines 12..12"), "{output}");
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
        output.contains("logo.png resource:image assets/logo.png"),
        "{output}"
    );
    assert!(
        output.contains("notes.txt resource:file assets/notes.txt"),
        "{output}"
    );
}

#[test]
fn items_claims_prefix_annotations_on_headings() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "items", "vault::troubleshoot"]);
    assert!(output.starts_with("2 items"), "{output}");
    let log = output
        .lines()
        .find(|line| line.starts_with("日志"))
        .expect("item line for 日志");
    assert!(log.contains("@(tag: urgent)"), "{output}");
    let manual = output
        .lines()
        .find(|line| line.starts_with("手册"))
        .expect("item line for 手册");
    assert!(manual.contains("scope L1"), "{output}");
    assert!(manual.contains("@(tag: urgent)"), "{output}");
}

#[test]
fn read_header_hands_off_module_identity_path_and_fingerprint() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "read", "vault::guide"]);
    assert!(output.contains("= 安装"), "{output}");
    // The module header is the identity-to-path bridge for the host editor.
    assert!(
        output.contains("module <vault::guide> guide.not lines 1.."),
        "{output}"
    );
    assert!(output.contains("fingerprint"), "{output}");
}

#[test]
fn locate_resolves_a_line_into_module_and_scope_breadcrumb() {
    let vault = fixture();
    let offset = GUIDE_NOT.find("概述").unwrap();
    // The line holding 概述 starts at some byte; derive its 1-based line.
    let line = GUIDE_NOT[..offset].lines().count();
    let output = run(
        &vault,
        &[
            "inspect",
            "locate",
            "guide.not",
            "--line",
            &line.to_string(),
        ],
    );
    assert!(output.contains("vault::guide"), "{output}");
    assert!(output.contains("安装"), "{output}");
    assert!(output.contains("--item 安装"), "{output}");
    assert!(output.contains(&format!("guide.not:{line}")), "{output}");
}

#[test]
fn locate_without_a_position_resolves_the_module_identity() {
    let vault = fixture();
    let output = run(&vault, &["inspect", "locate", "troubleshoot.not"]);
    assert!(output.contains("vault::troubleshoot"), "{output}");
    assert!(!output.contains("--item"), "{output}");
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
        &[
            "inspect",
            "references",
            "vault::guide",
            "--direction",
            "outgoing",
        ],
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
    assert!(output.contains("@!(status: draft)"), "{output}");
    assert!(output.contains("core::section \"安装\" L1"), "{output}");
    assert!(output.contains("core::section \"故障排除\" L2"), "{output}");
    assert!(output.contains("core::section \"后记\" L1"), "{output}");
}

#[test]
fn ancestors_item_selector_prints_the_named_scope_branch() {
    let vault = fixture();
    let output = run(
        &vault,
        &["inspect", "ancestors", "vault::troubleshoot/日志"],
    );
    assert!(output.contains("@!(severity: high)"), "{output}");
    assert!(output.contains("@(tag: urgent)"), "{output}");
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
        output.contains("core::section \"安装\" L1 lines 3..10"),
        "{output}"
    );
    assert!(output.contains("@(wip: true)"), "{output}");
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

#[test]
fn read_splits_a_range_into_uniform_attribute_segments() {
    let vault = fixture();
    let start = GUIDE_NOT.find("先读概述").unwrap();
    let end = GUIDE_NOT.find("完。").unwrap() + "完。".len();
    let output = run(
        &vault,
        &[
            "inspect",
            "read",
            "vault::guide",
            "--byte-range",
            &format!("{start}..{end}"),
        ],
    );
    // The module attribute merges into every segment's Dict; the wip
    // annotation holds only for the leading segment (后记's sibling lacks
    // it), so the two segments carry different Dicts.
    assert!(output.contains("(status: \"draft\", wip: true)"), "{output}");
    assert!(output.contains("(status: \"draft\")"), "{output}");
    assert!(output.contains("segments 2"), "{output}");
    assert!(output.contains("[1] <vault::guide/安装> lines 6..10 bytes 44..154"), "{output}");
    // Segment 2 starts on the boundary byte where 安装 (and its nested
    // 故障排除) end, so no addressable item contains its start.
    assert!(
        output.contains("[2] lines 10..14 bytes 154..172"),
        "{output}"
    );
    // Editing handoff: the header carries the module identity, the source
    // path, and the fingerprint.
    assert!(output.contains("module <vault::guide> guide.not"), "{output}");
    assert!(output.contains("fingerprint"), "{output}");
}

#[test]
fn read_point_reports_the_full_effective_environment() {
    let vault = fixture();
    let offset = GUIDE_NOT.find("概述").unwrap();
    let output = run(
        &vault,
        &[
            "inspect",
            "read",
            "vault::guide",
            "--offset",
            &offset.to_string(),
        ],
    );
    assert!(output.contains("segments 1"), "{output}");
    assert!(output.contains("(status: \"draft\", wip: true)"), "{output}");
    // The point resolves to the innermost node: the text run inside the
    // paragraph; the header container line is the single source for it.
    assert!(output.contains("container <anonymous>"), "{output}");
}

#[test]
fn read_line_range_validates_against_the_source() {
    let vault = fixture();
    let output = run(
        &vault,
        &["inspect", "read", "vault::guide", "--line", "5..7"],
    );
    assert!(output.contains("lines 5..7"), "{output}");
    let lines = GUIDE_NOT.lines().count();
    let failed = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("--no-daemon")
        .arg("--vault")
        .arg(vault.0.path())
        .args([
            "inspect",
            "read",
            "vault::guide",
            "--line",
            &format!("{}..{}", lines + 5, lines + 9),
        ])
        .output()
        .expect("failed to spawn the notist binary");
    assert!(!failed.status.success(), "out-of-range lines must fail");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("outside the selected source"));
}

#[test]
fn read_from_line_window_reads_to_the_end_and_clamps() {
    let vault = fixture();
    // Without --lines the window runs to the source end.
    let tail = run(
        &vault,
        &["inspect", "read", "vault::guide", "--from-line", "12"],
    );
    assert!(tail.contains("lines 12..14"), "{tail}");
    assert!(tail.contains("   14 | 完。"), "{tail}");
    // A count past the end clamps instead of erroring (unlike --line,
    // which validates strictly).
    let clamped = run(
        &vault,
        &["inspect", "read", "vault::guide", "--from-line", "12", "--lines", "100"],
    );
    assert!(clamped.contains("lines 12..14"), "{clamped}");
    // A bounded window inside the source selects exactly those lines.
    let window = run(
        &vault,
        &["inspect", "read", "vault::guide", "--from-line", "8", "--lines", "2"],
    );
    assert!(window.contains("lines 8..9"), "{window}");
    assert!(window.contains("    8 | == 故障排除"), "{window}");
}

#[test]
fn read_lines_requires_from_line() {
    let vault = fixture();
    let failed = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("--no-daemon")
        .arg("--vault")
        .arg(vault.0.path())
        .args(["inspect", "read", "vault::guide", "--lines", "3"])
        .output()
        .expect("failed to spawn the notist binary");
    assert!(!failed.status.success(), "--lines without --from-line must fail");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("--from-line"));
}

#[test]
fn read_byte_range_must_land_on_utf8_boundaries() {
    let vault = fixture();
    // One byte into the three-byte 概 character: not a UTF-8 boundary.
    let start = GUIDE_NOT.find("概述").unwrap() + 1;
    let end = GUIDE_NOT.len();
    let failed = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("--no-daemon")
        .arg("--vault")
        .arg(vault.0.path())
        .args([
            "inspect",
            "read",
            "vault::guide",
            "--byte-range",
            &format!("{start}..{end}"),
        ])
        .output()
        .expect("failed to spawn the notist binary");
    assert!(!failed.status.success(), "mid-character ranges must fail");
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.contains("invalid_argument"),
        "typed error, not a panic: {stderr}"
    );
    assert!(stderr.contains("UTF-8 boundaries"));
}

/// `docs/example/test.not` is a standing test case: every `$ notist …` line
/// in its trailing text block is an executable request. The fixture copies
/// the example directory into a temporary vault (keeping the module at
/// `vault::example::test`), rewrites the embedded `--vault docs` to the copy,
/// and runs every request.
#[test]
fn read_executes_the_requests_embedded_in_the_example_fixture() {
    let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/example");
    let text = std::fs::read_to_string(source_dir.join("test.not"))
        .expect("docs/example/test.not is part of the repo");
    let commands: Vec<String> = text
        .lines()
        .filter(|line| line.starts_with("$ notist "))
        .map(|line| line["$ notist ".len()..].to_owned())
        .collect();
    assert!(
        commands.len() >= 3,
        "the fixture should embed at least the range, point, and whole-module requests"
    );

    let vault = fixture();
    let example = vault.0.path().join("example");
    std::fs::create_dir_all(&example).unwrap();
    std::fs::copy(source_dir.join("test.not"), example.join("test.not")).unwrap();

    for command in &commands {
        let mut args = vec!["--no-daemon".to_owned()];
        let mut tokens = command.split_whitespace().peekable();
        while let Some(token) = tokens.next() {
            if token == "--vault" {
                // Rewrite the repo-root-relative docs path to the fixture
                // vault root so `vault::example::test` resolves.
                tokens.next();
                args.push("--vault".to_owned());
                args.push(vault.0.path().display().to_string());
            } else {
                args.push(token.to_owned());
            }
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
            .args(&args)
            .output()
            .expect("failed to spawn the notist binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "embedded request {command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.starts_with("module <vault::example::test>"),
            "embedded request {command:?} printed unexpected header:\n{stdout}"
        );

        if command.contains("--line 12..22") {
            assert!(stdout.contains("lines 12..22"), "{stdout}");
            // Blank lines at a scope boundary belong to the segment above:
            // the blank before @(b:) stays with Heading 2.1, and each of
            // Heading 2.2's segments opens on its declaring annotation.
            assert!(stdout.contains("segments 3"), "{stdout}");
            assert!(
                stdout.contains(
                    "[1] <vault::example::test/Heading 1/Heading 2.1> lines",
                ),
                "{stdout}"
            );
            assert!(stdout.contains("(b: \"b\", c: \"c\", o: \"o\")"), "{stdout}");
            // The segment's authored lines ride along (attribute-annotated
            // read), and the list-item boundary lands cleanly — no cut line.
            assert!(stdout.contains("   22 | - List<|"), "{stdout}");
        } else if command.contains("--offset") {
            assert!(stdout.contains("segments 1"), "{stdout}");
            assert!(stdout.contains("(a: \"a\", o: \"o\")"), "{stdout}");
        }
    }
}

#[test]
fn read_embeds_segment_content() {
    let vault = fixture();
    let start = GUIDE_NOT.find("先读概述").unwrap();
    let end = GUIDE_NOT.find("完。").unwrap() + "完。".len();
    let range = format!("{start}..{end}");
    let run_read = |extra: &[&str]| {
        let mut all = vec![
            "inspect",
            "read",
            "vault::guide",
            "--byte-range",
            range.as_str(),
        ];
        all.extend_from_slice(extra);
        run(&vault, &all)
    };
    let output = run_read(&[]);
    // Content is embedded per segment with the line-number gutter.
    assert!(output.contains("    6 | 先读概述"), "{output}");
    assert!(output.contains("   14 | 完。"), "{output}");
    let quiet = run_read(&["--attrs-only"]);
    assert!(!quiet.contains(" | "), "no gutter without content: {quiet}");
    assert!(!quiet.contains("先读概述"), "{quiet}");
}

#[test]
fn read_color_changes_only_the_rendering() {
    let vault = fixture();
    let plain = run(
        &vault,
        &["inspect", "read", "vault::guide", "--line", "5..7"],
    );
    assert!(
        !plain.contains('\x1b'),
        "piped output must be uncolored: {plain:?}"
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_notist"))
        .arg("--color")
        .arg("always")
        .arg("--no-daemon")
        .arg("--vault")
        .arg(vault.0.path())
        .args(["inspect", "read", "vault::guide", "--line", "5..7"])
        .output()
        .expect("failed to spawn the notist binary");
    let colored = String::from_utf8(output.stdout).unwrap();
    assert!(colored.contains('\x1b'), "always must color: {colored:?}");
    // Styling only: strip ANSI and the result is byte-identical.
    let mut stripped = String::with_capacity(colored.len());
    let mut chars = colored.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            stripped.push(c);
        }
    }
    assert_eq!(plain, stripped, "color must not change the logical result");
}
