mod common;

use common::{Client, Vault};
use lsp_types::{
    CompletionItemKind, CompletionResponse, CompletionTextEdit, HoverContents, Location, Position,
};
use serde_json::json;

#[test]
fn references_returns_cross_file_locations_through_the_real_loop() {
    // The referencing file must be the root module (`README.not`): a
    // `#<target>` link inside a nested module resolves to a child module.
    let vault = Vault::new(&[
        ("target.not", "= Target\n"),
        ("README.not", "see #<target> here\n"),
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
            "position": {"line": 0, "character": 8},
            "context": {"includeDeclaration": false}
        }),
    );
    let locations: Option<Vec<Location>> =
        serde_json::from_value(common::ok_result(client.await_response(outgoing)))
            .expect("references response shape");
    let locations = locations.expect("non-empty references without the definition");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri.as_str(), readme);
    assert_eq!(locations[0].range.start.line, 0);
    assert!(locations[0].range.start.character <= 8);
    assert!(locations[0].range.end.character > 8);
    assert_eq!(
        utf16_slice("see #<target> here\n", locations[0].range),
        "<target>"
    );

    let including_definition = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 8},
            "context": {"includeDeclaration": true}
        }),
    );
    let locations: Vec<Location> =
        serde_json::from_value::<Option<Vec<Location>>>(common::ok_result(
            client.await_response(including_definition),
        ))
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

#[test]
fn wide_character_lines_round_trip_utf16_positions_end_to_end() {
    // The root module (`README.not`) keeps the `#<target>` links resolvable
    // while exercising UTF-16 positions over emoji and CJK characters.
    let vault = Vault::new(&[
        ("target.not", "= Target\n"),
        ("README.not", "😀中 #<target>\n😀中 #<tar"),
    ]);
    let wide = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&wide, |_| true, "the baseline push");

    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": wide},
            "position": {"line": 0, "character": 8}
        }),
    );
    let hover: Option<lsp_types::Hover> =
        serde_json::from_value(common::ok_result(client.await_response(hover_id)))
            .expect("hover response shape");
    let hover = hover.expect("hover over the wiki reference");
    let range = hover.range.expect("hover range");
    assert_eq!(range.start, Position::new(0, 5));
    assert_eq!(range.end, Position::new(0, 13));
    assert!(matches!(
        hover.contents,
        HoverContents::Markup(ref markup) if markup.value.contains("target")
    ));

    let completion_id = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": wide},
            "position": {"line": 1, "character": 9}
        }),
    );
    let completion: Option<CompletionResponse> =
        serde_json::from_value(common::ok_result(client.await_response(completion_id)))
            .expect("completion response shape");
    let CompletionResponse::Array(items) = completion.expect("completion items") else {
        panic!("expected an array completion response");
    };
    assert!(items.iter().any(|item| item.kind == Some(CompletionItemKind::MODULE)));
    let candidate = items
        .iter()
        .find(|item| item.label == "target")
        .expect("`target` module completion");
    let CompletionTextEdit::Edit(edit) = candidate.text_edit.as_ref().expect("text edit") else {
        panic!("expected a plain text edit");
    };
    assert_eq!(edit.new_text, "target");
    assert_eq!(edit.range.start, Position::new(1, 6));
    assert_eq!(edit.range.end, Position::new(1, 9));
    assert_eq!(utf16_slice("😀中 #<tar", edit.range), "tar");

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

fn utf16_slice(line: &str, range: lsp_types::Range) -> String {
    let units: Vec<u16> = line.encode_utf16().collect();
    let start = range.start.character as usize;
    let end = range.end.character as usize;
    assert!(end <= units.len(), "range {range:?} exceeds the line");
    String::from_utf16(&units[start..end]).expect("valid UTF-16 slice")
}

#[test]
fn document_references_resolves_modules_without_position_ambiguity() {
    // `infra.not` opens with a heading, so offset 0 lands on the heading
    // symbol: this is exactly the case `textDocument/references` cannot
    // express (it returns null for the module), and the reason the
    // experimental document-level method takes the path as the selector.
    let vault = Vault::new(&[("infra.not", "# Infra\n"), ("README.not", "see #<infra> here\n")]);
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
    assert!(incoming["revision"].is_u64(), "revision is a freshness gate");
    let items = incoming["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1, "one incoming occurrence, no definition marker");
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
    assert!(rendered["revision"].is_u64(), "revision is a freshness gate");
    let page = &rendered["page"];
    assert_eq!(page["title"], "Hello");
    let fragment = page["fragment"].as_str().expect("fragment string");
    assert!(fragment.contains("Hello"), "heading text is rendered");
    assert!(fragment.contains("emphasized"), "inline markup text survives");
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
