mod common;

use common::{Client, Vault};
use serde_json::json;

#[test]
fn full_sync_did_change_updates_pushed_diagnostics() {
    let vault = Vault::new(&[("README.not", "ok\n"), ("child.not", "child\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);
    client.expect_diagnostics(&readme, |_| true, "the disk baseline push");

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": readme, "languageId": "notist", "version": 1, "text": "[[child]]"}
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
    assert!(broken
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message == "unknown function `broken`"));

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": readme, "version": 3},
            "contentChanges": [{"text": "[[child]]"}]
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
fn contract_violating_did_change_is_rejected_and_keeps_the_session_usable() {
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
    let violations = [
        (
            "an incremental edit",
            json!({
                "textDocument": {"uri": readme, "version": 2},
                "contentChanges": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}},
                    "text": "next"
                }]
            }),
        ),
        (
            "exactly one is required",
            json!({
                "textDocument": {"uri": readme, "version": 2},
                "contentChanges": [{"text": "first"}, {"text": "second"}]
            }),
        ),
        (
            "behind current",
            json!({
                "textDocument": {"uri": readme, "version": 0},
                "contentChanges": [{"text": "stale"}]
            }),
        ),
    ];
    for (expected_fragment, violation) in violations {
        client.notify("textDocument/didChange", violation);
        let logged = client.expect_log_message("rejected didChange");
        assert!(
            logged["message"]
                .as_str()
                .is_some_and(|message| message.contains(expected_fragment)),
            "warning for the rejection should name the cause `{expected_fragment}`: {logged}"
        );

        let probe = client.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": readme},
                "position": {"line": 0, "character": 0}
            }),
        );
        assert!(
            client.await_response(probe).response_result.unwrap().is_null(),
            "session should stay responsive after the rejection"
        );
    }

    assert!(client.wait_for_stderr("rejected didChange"));

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
