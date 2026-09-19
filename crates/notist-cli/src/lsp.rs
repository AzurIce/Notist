use lsp_server::{Connection, Message, Notification, Response};
use notist_analysis::{Workspace, offset, uri_path};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, threads) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let enc = params
        .pointer("/capabilities/general/positionEncodings")
        .and_then(Value::as_array);
    let utf8 = enc.is_some_and(|v| v.iter().any(|s| s == "utf-8"));
    let markdown_hover = params
        .pointer("/capabilities/textDocument/hover/contentFormat")
        .and_then(Value::as_array)
        .and_then(|formats| {
            formats
                .iter()
                .find(|format| *format == "markdown" || *format == "plaintext")
        })
        .is_some_and(|format| format == "markdown");
    let document_changes = params
        .pointer("/capabilities/workspace/workspaceEdit/documentChanges")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if enc.is_some_and(|v| !v.iter().any(|s| s == "utf-8" || s == "utf-16")) {
        connection.sender.send(Message::Response(Response::new_err(
            id,
            -32602,
            "client must support utf-8 or utf-16".into(),
        )))?;
        return Err("client must support utf-8 or utf-16".into());
    }
    connection.initialize_finish(id,json!({"capabilities":{"positionEncoding":if utf8{"utf-8"}else{"utf-16"},"workspace":{"workspaceFolders":{"supported":true,"changeNotifications":true}},"textDocumentSync":{"openClose":true,"change":2,"save":true},"documentSymbolProvider":true,"definitionProvider":true,"referencesProvider":true,"renameProvider":{"prepareProvider":true},"hoverProvider":true,"inlayHintProvider":true,"completionProvider":{"triggerCharacters":[":","[","#"]}},"serverInfo":{"name":"notist"}}))?;
    let mut workspace = Workspace::default();
    let mut roots = params["workspaceFolders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|folder| folder["uri"].as_str().and_then(uri_path))
        .collect::<Vec<_>>();
    if roots.is_empty() {
        roots.push(
            params["rootUri"]
                .as_str()
                .and_then(uri_path)
                .unwrap_or(std::env::current_dir()?),
        );
    }
    workspace.set_roots(roots);
    let mut published = BTreeMap::new();
    publish_workspace(&connection, &workspace, utf8, &mut published)?;
    let mut refreshed = Instant::now();
    loop {
        // Poll disk as a fallback for clients without watched-file notifications.
        if refreshed.elapsed() >= Duration::from_secs(2) {
            workspace.refresh();
            publish_workspace(&connection, &workspace, utf8, &mut published)?;
            refreshed = Instant::now();
        }
        let message = match connection.receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => message,
            Err(error) if error.is_timeout() => continue,
            Err(_) => break,
        };
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                let uri = request.params["textDocument"]["uri"].as_str().unwrap_or("");
                if matches!(
                    request.method.as_str(),
                    "textDocument/prepareRename" | "textDocument/rename"
                ) {
                    let outcome = if request.method == "textDocument/prepareRename" {
                        workspace.prepare_rename(uri, &request.params["position"], utf8)
                    } else {
                        workspace.rename(
                            uri,
                            &request.params["position"],
                            request.params["newName"].as_str().unwrap_or(""),
                            utf8,
                        )
                    };
                    match outcome {
                        Ok(mut result) => {
                            if request.method == "textDocument/rename" && !document_changes {
                                let mut changes = serde_json::Map::new();
                                for change in
                                    result["documentChanges"].as_array().into_iter().flatten()
                                {
                                    if let Some(uri) = change["textDocument"]["uri"].as_str() {
                                        changes.insert(uri.into(), change["edits"].clone());
                                    }
                                }
                                result = json!({"changes": changes});
                            }
                            connection
                                .sender
                                .send(Message::Response(Response::new_ok(request.id, result)))?;
                        }
                        Err(error) => connection.sender.send(Message::Response(
                            Response::new_err(request.id, -32602, error),
                        ))?,
                    }
                    continue;
                }
                let result = match request.method.as_str() {
                    "textDocument/inlayHint" => {
                        workspace.inlay_hints(uri, &request.params["range"], utf8)
                    }
                    "textDocument/documentSymbol" => workspace
                        .documents
                        .get(uri)
                        .map_or(json!([]), |d| d.symbols(utf8)),
                    "textDocument/completion" => {
                        workspace.completions_at(uri, &request.params["position"], utf8)
                    }
                    "textDocument/definition" => {
                        workspace.definition(uri, &request.params["position"], utf8)
                    }
                    "textDocument/hover" => workspace.hover_with_format(
                        uri,
                        &request.params["position"],
                        utf8,
                        markdown_hover,
                    ),
                    "textDocument/references" => workspace.references_with_declaration(
                        uri,
                        &request.params["position"],
                        utf8,
                        request.params["context"]["includeDeclaration"]
                            .as_bool()
                            .unwrap_or(false),
                    ),
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
                if n.method == "workspace/didChangeWorkspaceFolders" {
                    let mut roots = workspace.projects.roots.clone();
                    for removed in n.params["event"]["removed"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(path) = removed["uri"].as_str().and_then(uri_path) {
                            roots.remove(&path.canonicalize().unwrap_or(path));
                        }
                    }
                    for added in n.params["event"]["added"].as_array().into_iter().flatten() {
                        if let Some(path) = added["uri"].as_str().and_then(uri_path) {
                            roots.insert(path);
                        }
                    }
                    workspace.set_roots(roots);
                    publish_workspace(&connection, &workspace, utf8, &mut published)?;
                    continue;
                }
                if n.method == "workspace/didChangeWatchedFiles" {
                    workspace.refresh();
                    publish_workspace(&connection, &workspace, utf8, &mut published)?;
                    continue;
                }
                let Some(uri) = n.params["textDocument"]["uri"].as_str() else {
                    continue;
                };
                match n.method.as_str() {
                    "textDocument/didOpen" => {
                        workspace.open(
                            uri.into(),
                            n.params["textDocument"]["text"]
                                .as_str()
                                .unwrap_or("")
                                .into(),
                            n.params["textDocument"]["version"].as_i64(),
                        );
                        workspace.refresh();
                    }
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
                        }
                        workspace.refresh();
                    }
                    "textDocument/didSave" => workspace.refresh(),
                    _ => continue,
                }
                publish_workspace(&connection, &workspace, utf8, &mut published)?;
            }
            _ => {}
        }
    }
    drop(connection);
    threads.join()?;
    Ok(())
}
fn publish_workspace(
    c: &Connection,
    workspace: &Workspace,
    utf8: bool,
    previous: &mut BTreeMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut next = BTreeMap::new();
    for (uri, doc) in &workspace.documents {
        next.insert(
            uri.clone(),
            json!({"uri":uri,"version":doc.version,"diagnostics":workspace.diagnostics(uri, utf8)}),
        );
    }
    for (uri, errors) in &workspace.projects.errors {
        let diagnostics = errors.iter().map(|message| json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":1,"source":"notist","message":message})).collect::<Vec<_>>();
        next.insert(uri.clone(), json!({"uri":uri,"diagnostics":diagnostics}));
    }
    for (uri, params) in &next {
        if previous.get(uri) != Some(params) {
            c.sender.send(Message::Notification(Notification::new(
                "textDocument/publishDiagnostics".into(),
                params.clone(),
            )))?;
        }
    }
    for uri in previous.keys().filter(|uri| !next.contains_key(*uri)) {
        c.sender.send(Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".into(),
            json!({"uri":uri,"diagnostics":[]}),
        )))?;
    }
    *previous = next;
    Ok(())
}
