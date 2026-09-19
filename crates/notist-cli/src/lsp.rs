use lsp_server::{Connection, Message, Notification, Response};
use notist_analysis::{Workspace, offset, uri_path};
use serde_json::{Value, json};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, threads) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let enc = params
        .pointer("/capabilities/general/positionEncodings")
        .and_then(Value::as_array);
    let utf8 = enc.is_some_and(|v| v.iter().any(|s| s == "utf-8"));
    if enc.is_some_and(|v| !v.iter().any(|s| s == "utf-8" || s == "utf-16")) {
        connection.sender.send(Message::Response(Response::new_err(
            id,
            -32602,
            "client must support utf-8 or utf-16".into(),
        )))?;
        return Err("client must support utf-8 or utf-16".into());
    }
    connection.initialize_finish(id,json!({"capabilities":{"positionEncoding":if utf8{"utf-8"}else{"utf-16"},"textDocumentSync":{"openClose":true,"change":2,"save":true},"documentSymbolProvider":true,"definitionProvider":true,"hoverProvider":true,"completionProvider":{"triggerCharacters":[":","[","#"]}},"serverInfo":{"name":"notist"}}))?;
    let mut workspace = Workspace::default();
    if let Some(root) = params["rootUri"].as_str().and_then(uri_path) {
        workspace.load(&root);
    }
    for (uri, doc) in &workspace.documents {
        publish(&connection, uri, doc, utf8)?;
    }
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                let uri = request.params["textDocument"]["uri"].as_str().unwrap_or("");
                let result = match request.method.as_str() {
                    "textDocument/documentSymbol" => workspace
                        .documents
                        .get(uri)
                        .map_or(json!([]), |d| d.symbols(utf8)),
                    "textDocument/completion" => workspace.completions(uri),
                    "textDocument/definition" => {
                        workspace.definition(uri, &request.params["position"], utf8)
                    }
                    "textDocument/hover" => {
                        let target = workspace.definition(uri, &request.params["position"], utf8);
                        if target.is_null() {
                            Value::Null
                        } else {
                            json!({"contents":{"kind":"plaintext","value":format!("let {}",workspace.word(uri,&request.params["position"],utf8).unwrap_or_default())}})
                        }
                    }
                    _ => {
                        connection.sender.send(Message::Response(Response::new_err(
                            request.id,
                            -32601,
                            "method not supported".into(),
                        )))?;
                        continue;
                    }
                };
                connection
                    .sender
                    .send(Message::Response(Response::new_ok(request.id, result)))?;
            }
            Message::Notification(n) => {
                let Some(uri) = n.params["textDocument"]["uri"].as_str() else {
                    continue;
                };
                match n.method.as_str() {
                    "textDocument/didOpen" => workspace.open(
                        uri.into(),
                        n.params["textDocument"]["text"]
                            .as_str()
                            .unwrap_or("")
                            .into(),
                        n.params["textDocument"]["version"].as_i64(),
                    ),
                    "textDocument/didChange" => {
                        let Some(doc) =
                            workspace.documents.get(uri).filter(|d| d.version.is_some())
                        else {
                            continue;
                        };
                        let Some(version) = n.params["textDocument"]["version"].as_i64() else {
                            continue;
                        };
                        if doc.version.is_some_and(|previous| version <= previous) {
                            continue;
                        }
                        let mut text = doc.text.clone();
                        for change in n.params["contentChanges"].as_array().into_iter().flatten() {
                            let new = change["text"].as_str().unwrap_or("");
                            if change["range"].is_object() {
                                let start = offset(&text, &change["range"]["start"], utf8);
                                let end = offset(&text, &change["range"]["end"], utf8);
                                if start <= end {
                                    text.replace_range(start..end, new);
                                }
                            } else {
                                text = new.into();
                            }
                        }
                        workspace.open(uri.into(), text, Some(version));
                    }
                    "textDocument/didClose" => {
                        workspace.documents.remove(uri);
                        if let Some(path) = uri_path(uri)
                            && let Ok(text) = std::fs::read_to_string(path)
                        {
                            workspace.open(uri.into(), text, None);
                        } else {
                            connection
                                .sender
                                .send(Message::Notification(Notification::new(
                                    "textDocument/publishDiagnostics".into(),
                                    json!({"uri":uri,"diagnostics":[]}),
                                )))?;
                        }
                    }
                    _ => continue,
                }
                if let Some(doc) = workspace.documents.get(uri) {
                    publish(&connection, uri, doc, utf8)?;
                }
            }
            _ => {}
        }
    }
    drop(connection);
    threads.join()?;
    Ok(())
}
fn publish(
    c: &Connection,
    uri: &str,
    doc: &notist_analysis::Document,
    utf8: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    c.sender.send(Message::Notification(Notification::new(
        "textDocument/publishDiagnostics".into(),
        json!({"uri":uri,"version":doc.version,"diagnostics":doc.diagnostics(utf8)}),
    )))?;
    Ok(())
}
