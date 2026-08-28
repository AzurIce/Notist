mod common;

use common::{Client, Vault};
use lsp_types::DiagnosticSeverity;
use serde_json::json;

#[test]
fn initialize_advertises_the_documented_capabilities_and_clean_shutdown_exits_successfully() {
    let vault = Vault::new(&[("README.not", "= Hello\n")]);
    let mut client = Client::spawn(&vault);
    let capabilities = client.initialize(&vault);

    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert_eq!(capabilities["textDocumentSync"], 1);
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
