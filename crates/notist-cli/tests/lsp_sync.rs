mod common;

use common::{Client, Vault};
use serde_json::json;

#[test]
fn whole_document_did_change_updates_pushed_diagnostics() {
    let vault = Vault::new(&[("README.not", "ok\n"), ("child.not", "child\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "#<child>"}
        }),
    );

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": readme, "version": 2},
            "contentChanges": [{"text": "#broken[]"}]
        }),
    );
    let broken = client.expect_diagnostics(
        &readme,
        |params| params.version == Some(2) && !params.diagnostics.is_empty(),
        "diagnostics for the edited overlay content",
    );
    assert_eq!(broken.version, Some(2));
    assert!(
        broken
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "unknown function `broken`")
    );

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": readme, "version": 3},
            "contentChanges": [{"text": "#<child>"}]
        }),
    );
    let repaired = client.expect_diagnostics(
        &readme,
        |params| params.version == Some(3),
        "the cleared diagnostics push after the fix",
    );
    assert_eq!(repaired.version, Some(3));

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn unroutable_did_change_is_rejected_and_keeps_the_session_usable() {
    // Contract violations no longer exist on the incremental surface: ranged
    // edits, batches, and out-of-order versions all apply. Two didChange
    // shapes remain unroutable and must fail loudly without killing the
    // session: a URI that cannot name a vault path, and a change for a
    // document that was never opened.
    let vault = Vault::new(&[("README.not", "ok\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    let unroutable_uri = "untitled:Untitled-1";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": unroutable_uri, "version": 2},
            "contentChanges": [{"text": "orphan"}]
        }),
    );
    let logged = client.expect_log_message("rejected textDocument/didChange");
    assert!(
        logged["message"]
            .as_str()
            .is_some_and(|message| message.contains(unroutable_uri)),
        "warning for the rejection should name the document: {logged}"
    );

    let unopened = vault.uri("README.not");
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": unopened, "version": 2},
            "contentChanges": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}},
                "text": "ghost"
            }]
        }),
    );
    assert!(
        client.wait_for_stderr("unexpected didChange"),
        "dropped unopened-document changes are noted on stderr"
    );

    // The session stays fully responsive after both rejections.
    let probe = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 0}
        }),
    );
    assert!(
        client
            .await_response(probe)
            .response_result
            .unwrap()
            .is_null(),
        "session should stay responsive after the rejections"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn malformed_notifications_are_contained_and_keep_the_session_alive() {
    let vault = Vault::new(&[("README.not", "ok\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "keep"}
        }),
    );

    // A decode failure must not take the server down ...
    client.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": readme, "version": 2}, "contentChanges": 42}),
    );
    assert!(
        client.wait_for_stderr("dropped malformed"),
        "dropped malformed messages are noted on stderr"
    );
    // ... nor a malformed cancel ...
    client.notify("$/cancelRequest", json!(42));
    // ... nor an unroutable didOpen, which additionally warns the client.
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": "untitled:Untitled-1", "languageId": "notist", "version": 1, "text": "x"}
        }),
    );
    let logged = client.expect_log_message("rejected textDocument/didOpen");
    assert!(
        logged["message"]
            .as_str()
            .is_some_and(|message| message.contains("untitled")),
        "warning should name the unroutable document: {logged}"
    );

    // The session still answers, and the accepted buffer is intact.
    let probe = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 0}
        }),
    );
    assert!(
        client
            .await_response(probe)
            .response_result
            .unwrap()
            .is_null(),
        "session should stay responsive after the malformed messages"
    );

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn daemon_mode_serves_the_full_sync_contract() {
    // Dual-mode coverage (see designs/lsp/test.not): the daemon path runs the
    // client through a multiplexed connection to a shared daemon process. The
    // sync contract must be identical to embedded mode.
    let vault = Vault::new(&[("README.not", "ok\n"), ("target.not", "= Target\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn_daemon(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "keep #<target> here"}
        }),
    );
    client.did_change(
        &readme,
        2,
        json!([
            {
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 4}
                },
                "text": "edited"
            },
            {
                "range": {
                    "start": {"line": 0, "character": 25},
                    "end": {"line": 0, "character": 25}
                },
                "text": " #missing[]"
            }
        ]),
    );
    // The new diagnostic is the readiness gate: once it arrives, the overlay
    // rebuild has been applied and queries hit the edited buffer.
    client.expect_diagnostics(
        &readme,
        |params| {
            params
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "unknown function `missing`")
        },
        "the appended diagnostic over the daemon path",
    );

    // The overlay answers through the daemon: with `edited` replacing
    // `keep`, the hover range covers the inner `<target>` link at 8..16.
    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 11}
        }),
    );
    let hover: Option<lsp_types::Hover> =
        serde_json::from_value(common::ok_result(client.await_response(hover_id)))
            .expect("hover response shape");
    let hover = hover.expect("hover over the edited reference");
    let range = hover.range.expect("hover range");
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 8);
    assert_eq!(range.end.character, 16);

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn watcher_rebuilds_preserve_open_overlays() {
    // A watcher-triggered rebuild (external disk edit) must not disturb the
    // open document's overlay: the editor's unsaved buffer wins over disk
    // even though the rebuild refreshes the disk sources.
    let vault = Vault::new(&[
        ("README.not", "disk #missing[]\n"),
        ("other.not", "other\n"),
        ("target.not", "= Target\n"),
    ]);
    let readme = vault.uri("README.not");
    let other = vault.uri("other.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1,
                             "text": "keep #<target> here"}
        }),
    );
    // The overlay has no diagnostics: the clearing push proves the didOpen
    // rebuild replaced the disk view.
    client.expect_diagnostics(
        &readme,
        |params| params.diagnostics.is_empty(),
        "the overlay clearing push after didOpen",
    );

    // External disk edit to a file WITHOUT an overlay triggers a watcher
    // rebuild whose overlay delta is empty — the path that historically
    // wiped the overlay set. The new diagnostic doubles as the gate proving
    // the watcher rebuild has been applied.
    vault.write_over("other.not", "broken #nonexistent[]\n");
    client.expect_diagnostics(
        &other,
        |params| {
            params
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "unknown function `nonexistent`")
        },
        "the watcher rebuild after the external disk edit",
    );

    // The overlay survived the watcher rebuild: hover still resolves the
    // unsaved buffer's `#<target>` link, not the disk text.
    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": readme},
            "position": {"line": 0, "character": 11}
        }),
    );
    let hover: Option<lsp_types::Hover> =
        serde_json::from_value(common::ok_result(client.await_response(hover_id)))
            .expect("hover response shape");
    let hover = hover.expect("hover over the overlay link after the watcher rebuild");
    assert!(matches!(
        hover.contents,
        lsp_types::HoverContents::Markup(ref markup) if markup.value.contains("target")
    ));

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn did_close_publishes_empty_diagnostics_for_the_closed_document() {
    let vault = Vault::new(&[("README.not", "clean\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "#missing[]"}
        }),
    );
    client.expect_diagnostics(
        &readme,
        |params| !params.diagnostics.is_empty(),
        "overlay diagnostics before the close",
    );

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": readme}}),
    );
    let cleared = client.expect_diagnostics(
        &readme,
        |params| params.diagnostics.is_empty(),
        "the empty clearing push after didClose",
    );
    assert_eq!(cleared.version, None);

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}
