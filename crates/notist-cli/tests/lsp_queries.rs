mod common;

use common::{Client, Vault};
use lsp_types::{Location, Position};
use serde_json::json;

#[test]
fn references_returns_cross_file_locations_through_the_real_loop() {
    // The referencing file must be the root module (`README.not`): a
    // `#<target>` link inside a nested module resolves to a child module.
    // The CJK prefix makes byte columns diverge from UTF-16 columns, so the
    // assertions below only hold if the session truly speaks utf-8 bytes.
    let vault = Vault::new(&[
        ("target.not", "= Target\n"),
        ("README.not", "看中 #<target> 文\n"),
    ]);
    let readme = vault.uri("README.not");
    let target = vault.uri("target.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the baseline push");

    let outgoing = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 11},
            "context": {"includeDeclaration": false}
        }),
    );
    let locations: Option<Vec<Location>> =
        serde_json::from_value(common::ok_result(client.await_response(outgoing)))
            .expect("references response shape");
    let locations = locations.expect("non-empty references without the definition");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri.as_str(), readme);
    // Byte columns: `<target>` spans bytes 8..16 (utf-16 columns would be
    // 4..12); utf-16 columns would fail these assertions.
    assert_eq!(locations[0].range.start, Position::new(0, 8));
    assert_eq!(locations[0].range.end, Position::new(0, 16));
    assert_eq!(
        utf8_slice("看中 #<target> 文\n", locations[0].range),
        "<target>"
    );

    let including_definition = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 11},
            "context": {"includeDeclaration": true}
        }),
    );
    let locations: Vec<Location> = serde_json::from_value::<Option<Vec<Location>>>(
        common::ok_result(client.await_response(including_definition)),
    )
    .expect("references response shape")
    .expect("references including the definition");
    assert_eq!(locations.len(), 2);
    let definition = locations
        .iter()
        .find(|location| location.uri.as_str() == target)
        .expect("definition location in target.not");
    assert_eq!(definition.range.start, Position::new(0, 0));
    assert_eq!(definition.range.end, Position::new(0, 0));

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

/// Byte-column slice for utf-8 sessions: range characters are byte offsets
/// within the line.
fn utf8_slice(line: &str, range: lsp_types::Range) -> String {
    let start = range.start.character as usize;
    let end = range.end.character as usize;
    assert!(
        line.is_char_boundary(start) && end <= line.len(),
        "range {range:?} exceeds the line"
    );
    line[start..end].to_owned()
}

/// UTF-16 code-unit slice for utf-16 sessions: range characters are UTF-16
/// offsets within the line.
fn utf16_slice(line: &str, range: lsp_types::Range) -> String {
    let units: Vec<u16> = line.encode_utf16().collect();
    let start = range.start.character as usize;
    let end = range.end.character as usize;
    assert!(end <= units.len(), "range {range:?} exceeds the line");
    String::from_utf16(&units[start..end]).expect("valid UTF-16 slice")
}

#[test]
fn utf16_sessions_convert_positions_on_cjk_and_emoji_documents() {
    // Every position below is computed in UTF-16 columns; a server secretly
    // speaking bytes fails these assertions (the CJK/emoji prefixes make the
    // two column spaces diverge).
    let readme_text = "看😀 #<target> 文\n#heading[]\n看😀 #<t\n";
    let vault = Vault::new(&[("target.not", "= 目标\n"), ("README.not", readme_text)]);
    let readme = vault.uri("README.not");
    let target = vault.uri("target.not");
    let mut client = Client::spawn(&vault);
    let capabilities = client.initialize_with_encodings(&vault, json!(["utf-16"]));
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    client.expect_diagnostics(&readme, |_| true, "the baseline push");

    // Open the document (disk content == overlay, so no rebuild race):
    // inbound conversions take the adapter's local text.
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": readme_text}
        }),
    );

    // Line 0 UTF-16 columns: 看=0, 😀=1..3, space=3, #=4, <=5, target=6..12,
    // >=12, space=13, 文=14.
    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 1, "character": 3}
        }),
    );
    let hover = common::ok_result(client.await_response(hover_id));
    assert!(
        hover["contents"]["value"]
            .as_str()
            .expect("hover markdown")
            .contains("#heading"),
        "hover over `heading` on line 1: {hover:?}"
    );

    // `<` at UTF-16 column 5 resolves only if the inbound column is read as
    // UTF-16 (byte column 5 lands mid-😀, outside the reference).
    let definition_id = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 5}
        }),
    );
    let definition: Option<Location> =
        serde_json::from_value(common::ok_result(client.await_response(definition_id)))
            .expect("definition response shape");
    let definition = definition.expect("definition of #<target> from UTF-16 column 5");
    assert_eq!(definition.uri.as_str(), target);
    assert_eq!(definition.range.start, Position::new(0, 0));

    // 文 at UTF-16 column 14 is outside the reference (byte column 14 would
    // land inside it).
    let outside_id = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 14}
        }),
    );
    let outside = common::ok_result(client.await_response(outside_id));
    assert!(outside.is_null(), "no definition on `文`: {outside:?}");

    // Outbound ranges are UTF-16 too: `<target>` spans columns 5..13 (byte
    // columns would be 9..17).
    let references_id = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 8},
            "context": {"includeDeclaration": false}
        }),
    );
    let locations: Vec<Location> = serde_json::from_value::<Option<Vec<Location>>>(
        common::ok_result(client.await_response(references_id)),
    )
    .expect("references response shape")
    .expect("references for #<target>");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri.as_str(), readme);
    assert_eq!(locations[0].range.start, Position::new(0, 5));
    assert_eq!(locations[0].range.end, Position::new(0, 13));
    assert_eq!(
        utf16_slice("看😀 #<target> 文", locations[0].range),
        "<target>"
    );

    // Completion after `#<t` on line 2 (UTF-16 column 7, mid-😀 as a byte
    // column) offers the target module with a UTF-16 replacement range.
    let completion_id = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 2, "character": 7}
        }),
    );
    let completion = common::ok_result(client.await_response(completion_id));
    let items = completion.as_array().expect("completion array");
    let item = items
        .iter()
        .find(|item| item["label"] == "target")
        .expect("completion offers the target module");
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line": 2, "character": 6}),
        "the replacement range is in UTF-16 columns"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn queries_right_after_did_change_wait_for_the_snapshot_to_catch_up() {
    // A didChange immediately followed by hover/completion (no waiting for
    // the diagnostics round-trip) used to resolve against the pre-edit
    // snapshot, answering with misplaced positions. The pinned fingerprint
    // makes the worker wait for the build carrying the edit and retry, so
    // the answer is always against the current text.
    let vault = Vault::new(&[("child.not", "child\n"), ("README.not", "alpha beta\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the baseline push");

    // Open with a diagnostic-bearing text and wait for its push: that build
    // is then known to be applied, so the edit below has exactly one
    // outstanding build — the one carrying it — and the pinned retry is
    // deterministic.
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "#missing[]\n"}
        }),
    );
    client.expect_diagnostics(
        &readme,
        |params| params
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown function")),
        "the diagnostics push for the opened overlay",
    );

    let edited = "#let greeting = \"hi\"\n#greeting\n#<ch\n";
    client.did_change(&readme, 2, json!([{"text": edited}]));

    // Immediately, without any diagnostics wait: hover over the binding the
    // edit just introduced. The pre-edit text has no line 1 at all, so a
    // stale-snapshot answer could never be this one.
    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 1, "character": 2}
        }),
    );
    let hover = common::ok_result(client.await_response(hover_id));
    let value = hover["contents"]["value"]
        .as_str()
        .expect("hover markdown over the edited `greeting`");
    assert!(value.contains("greeting"), "{value}");
    assert_eq!(hover["range"]["start"]["line"], 1);

    // Completion after `#<ch` offers the sibling module, again only
    // computable from the edited text.
    let completion_id = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 2, "character": 3}
        }),
    );
    let completion = common::ok_result(client.await_response(completion_id));
    let items = completion.as_array().expect("completion array");
    assert!(
        items.iter().any(|item| item["label"] == "child"),
        "{items:?}"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn document_references_resolves_modules_without_position_ambiguity() {
    // `infra.not` opens with a heading, so offset 0 lands on the heading
    // symbol: this is exactly the case `textDocument/references` cannot
    // express (it returns null for the module), and the reason the
    // experimental document-level method takes the path as the selector.
    let vault = Vault::new(&[
        ("infra.not", "# Infra\n"),
        ("README.not", "see #<infra> here\n"),
    ]);
    let infra = vault.uri("infra.not");
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the baseline push");

    let incoming_id = client.request(
        "notist/documentReferences",
        json!({
            "textDocument": {"uri": infra},
            "direction": "incoming"
        }),
    );
    let incoming = common::ok_result(client.await_response(incoming_id));
    assert!(
        incoming["revision"].is_u64(),
        "revision is a freshness gate"
    );
    let items = incoming["items"].as_array().expect("items array");
    assert_eq!(
        items.len(),
        1,
        "one incoming occurrence, no definition marker"
    );
    assert_eq!(items[0]["direction"], "incoming");
    assert_eq!(items[0]["targetModule"], "vault::infra");
    assert_eq!(items[0]["uri"].as_str().expect("uri"), readme);
    assert_eq!(items[0]["isDefinition"], false);

    let both_id = client.request(
        "notist/documentReferences",
        json!({
            "textDocument": {"uri": readme},
            "direction": "both",
            "includeDefinition": true
        }),
    );
    let both = common::ok_result(client.await_response(both_id));
    let items = both["items"].as_array().expect("items array");
    let outgoing = items
        .iter()
        .find(|item| item["direction"] == "outgoing")
        .expect("outgoing occurrence from README");
    assert_eq!(outgoing["targetModule"], "vault::infra");
    assert_eq!(outgoing["targetKind"], "module");
    assert!(outgoing["url"].as_str().expect("raw url").contains("infra"));

    let definition = items
        .iter()
        .find(|item| item["isDefinition"] == true)
        .expect("definition marker with includeDefinition");
    assert_eq!(
        definition["uri"].as_str().expect("uri"),
        readme,
        "the queried module is README itself"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn render_document_returns_the_evaluated_fragment_and_module_resources() {
    // The fragment comes from the same evaluated pipeline as the preview
    // site: markup renders without source markers, and the module's resource
    // files travel alongside for the consumer's URL rewriting. A path that
    // backs no module (a resource file here) renders to null.
    let vault = Vault::new(&[
        ("README.not", "= Hello\n*emphasized* text\n"),
        ("pic.png", "not really a png"),
    ]);
    let readme = vault.uri("README.not");
    let picture = vault.uri("pic.png");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the baseline push");

    let render_id = client.request(
        "notist/renderDocument",
        json!({ "textDocument": {"uri": readme} }),
    );
    let rendered = common::ok_result(client.await_response(render_id));
    assert!(
        rendered["revision"].is_u64(),
        "revision is a freshness gate"
    );
    let page = &rendered["page"];
    assert_eq!(page["title"], "Hello");
    let fragment = page["fragment"].as_str().expect("fragment string");
    assert!(fragment.contains("Hello"), "heading text is rendered");
    assert!(
        fragment.contains("emphasized"),
        "inline markup text survives"
    );
    assert!(!fragment.contains("= Hello"), "source markers do not leak");
    let resources = rendered["resources"].as_array().expect("resources array");
    assert!(
        resources
            .iter()
            .any(|resource| resource["name"] == "pic.png"),
        "the module's resources travel with the page"
    );

    let null_id = client.request(
        "notist/renderDocument",
        json!({ "textDocument": {"uri": picture} }),
    );
    assert!(
        common::ok_result(client.await_response(null_id)).is_null(),
        "non-source files have no module to render"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}
