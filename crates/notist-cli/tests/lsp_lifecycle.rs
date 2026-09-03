mod common;

use common::{Client, Vault};
use lsp_types::DiagnosticSeverity;
use serde_json::json;

#[test]
fn initialize_advertises_the_documented_capabilities_and_clean_shutdown_exits_successfully() {
    let vault = Vault::new(&[("README.not", "= Hello\n")]);
    let mut client = Client::spawn(&vault);
    let capabilities = client.initialize(&vault);

    let encoding = capabilities["positionEncoding"].as_str().unwrap();
    assert_eq!(encoding, "utf-8");
    let sync = &capabilities["textDocumentSync"];
    assert_eq!(sync["openClose"], true);
    assert_eq!(sync["change"], 2, "TextDocumentSyncKind::INCREMENTAL");
    assert_eq!(sync["save"], true, "didSave support is declared");
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"],
        json!(["[", ":", "#", "(", ",", "<", "/"])
    );
    assert_eq!(capabilities["completionProvider"]["resolveProvider"], false);
    assert_eq!(capabilities["hoverProvider"], true);
    assert_eq!(capabilities["definitionProvider"], true);
    assert_eq!(capabilities["referencesProvider"], true);
    assert_eq!(capabilities["documentSymbolProvider"], true);
    assert_eq!(capabilities["workspaceSymbolProvider"], true);

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn utf16_only_clients_get_a_utf16_session() {
    let vault = Vault::new(&[("README.not", "ok\n")]);
    let mut client = Client::spawn(&vault);
    let capabilities = client.initialize_with_encodings(&vault, json!(["utf-16"]));

    let encoding = capabilities["positionEncoding"].as_str().unwrap();
    assert_eq!(encoding, "utf-16");

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn clients_offering_neither_utf8_nor_utf16_are_rejected_at_initialize() {
    // The wire protocol speaks utf-8 or utf-16. A client offering neither
    // gets a loud refusal instead of a session whose every position is
    // misread.
    let vault = Vault::new(&[("README.not", "ok\n")]);
    let mut client = Client::spawn(&vault);
    let id = client.request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": vault.root_uri(),
            "capabilities": { "general": { "positionEncodings": ["utf-32"] } },
            "workspaceFolders": [{"uri": vault.root_uri(), "name": "vault"}],
        }),
    );
    let response = client.await_response(id);
    let error = response
        .response_result
        .expect_err("utf-32-only clients must be refused");
    assert_eq!(error.code, -32803, "RequestFailed");
    assert!(
        error.message.contains("utf-8") && error.message.contains("utf-16"),
        "rejection should name the supported encodings: {error:?}"
    );
    assert!(
        client.wait_for_stderr("position encoding"),
        "the refusal is also logged on stderr"
    );
}

#[test]
fn baseline_diagnostics_are_pushed_without_any_client_document_activity() {
    let vault = Vault::new(&[("README.not", "#missing[]\n")]);
    let readme = vault.uri("README.not");
    let mut client = Client::spawn(&vault);
    client.initialize(&vault);

    let params = client.expect_diagnostics(
        &readme,
        |_| true,
        "the spontaneous baseline diagnostics push",
    );
    assert_eq!(params.version, None);
    assert_eq!(params.diagnostics.len(), 1);
    let diagnostic = &params.diagnostics[0];
    assert_eq!(diagnostic.message, "unknown function `missing`");
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(diagnostic.source.as_deref(), Some("notist"));

    let status = client.shutdown_and_exit();
    assert_eq!(status.code(), Some(0));
}
