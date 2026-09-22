use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::Duration,
};

struct Client {
    _root: tempfile::TempDir,
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
}
impl Client {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_notist"))
            .arg("lsp")
            .current_dir(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, output) = channel();
        std::thread::spawn(move || {
            loop {
                let mut size = 0;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length: ") {
                        size = value.trim().parse().unwrap();
                    }
                }
                let mut bytes = vec![0; size];
                if stdout.read_exact(&mut bytes).is_err() {
                    return;
                }
                if tx.send(serde_json::from_slice(&bytes).unwrap()).is_err() {
                    return;
                }
            }
        });
        Self {
            _root: root,
            child,
            input: Some(input),
            output,
        }
    }
    fn send(&mut self, message: Value) {
        let body = message.to_string();
        let input = self.input.as_mut().unwrap();
        write!(input, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        input.flush().unwrap();
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(10))
            .expect("LSP response timed out")
    }
    fn response(&self, id: i64) -> Value {
        for _ in 0..100 {
            let message = self.receive();
            if message["id"] == id {
                return message;
            }
        }
        panic!("too many notifications before response");
    }
}

#[test]
fn discovers_packages_from_cwd_and_shares_dependency_edits() {
    use std::fs;
    let mut client = Client::new();
    let root = client._root.path().canonicalize().unwrap();
    for name in ["a", "nested/b"] {
        let path = root.join(name);
        fs::create_dir_all(path.join("docs")).unwrap();
        let deps = if name == "a" {
            ""
        } else {
            "[dependencies.a]\npath = '../../a'"
        };
        fs::write(
            path.join("Notist.toml"),
            format!("[package]\nname = 'test'\n{deps}"),
        )
        .unwrap();
        fs::write(
            path.join("docs/README.notc"),
            if name == "a" {
                "let value = 1;"
            } else {
                "use a::value;\nvalue;"
            },
        )
        .unwrap();
    }
    let au = notist_analysis::file_uri(&root.join("a/docs/README.notc"));
    let bu = notist_analysis::file_uri(&root.join("nested/b/docs/README.notc"));
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    assert_eq!(
        client.receive()["result"]["capabilities"]["workspace"]["workspaceFolders"]["supported"],
        true
    );
    client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    let baseline = [client.receive(), client.receive()];
    assert!(baseline.iter().any(|v| v["params"]["uri"] == au));
    assert!(baseline.iter().any(|v| v["params"]["uri"] == bu));
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert_eq!(client.response(2)["result"]["uri"], au);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":au,"version":1,"languageId":"notist-code","text":"\nlet value = 2;"}}}));
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert_eq!(client.response(3)["result"]["range"]["start"]["line"], 1);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":au}}}));
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert_eq!(client.response(4)["result"]["range"]["start"]["line"], 0);
    fs::write(root.join("a/docs/README.notc"), "\n\nlet value = 3;").unwrap();
    client.send(json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":au,"type":2}]}}));
    client.send(json!({"jsonrpc":"2.0","id":40,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert_eq!(client.response(40)["result"]["range"]["start"]["line"], 2);
    client.send(json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[],"removed":[{"uri":notist_analysis::file_uri(&root),"name":"root"}]}}}));
    client.send(json!({"jsonrpc":"2.0","id":5,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert!(client.response(5)["result"].is_null());
    client.send(json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"removed":[],"added":[{"uri":notist_analysis::file_uri(&root),"name":"root"}]}}}));
    client.send(json!({"jsonrpc":"2.0","id":6,"method":"textDocument/definition","params":{"textDocument":{"uri":bu},"position":{"line":1,"character":2}}}));
    assert_eq!(client.response(6)["result"]["uri"], au);
    client.send(json!({"jsonrpc":"2.0","id":7,"method":"shutdown","params":null}));
    assert_eq!(client.response(7)["id"], 7);
    client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    drop(client.input.take());
    assert!(client.child.wait().unwrap().success());
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn label_navigation_and_diagnostics_follow_editor_overlays() {
    use std::fs;
    let mut client = Client::new();
    let root = client._root.path().canonicalize().unwrap();
    fs::create_dir(root.join("docs")).unwrap();
    fs::write(root.join("Notist.toml"), "[package]\nname = 'labels'\n").unwrap();
    fs::write(
        root.join("docs/README.not"),
        "[[vault::guide::\"A\"::\"例子\"]]",
    )
    .unwrap();
    fs::write(
        root.join("docs/guide.not"),
        "= A\n== Middle\n=== 例子\nText",
    )
    .unwrap();
    let uri = notist_analysis::file_uri(&root.join("docs/README.not"));
    let guide = notist_analysis::file_uri(&root.join("docs/guide.not"));
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    client.response(1);
    client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":22}}}));
    let definition = client.response(2);
    assert_eq!(definition["result"]["uri"], guide);
    assert_eq!(definition["result"]["range"]["start"]["line"], 2);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":guide,"version":1,"languageId":"notist","text":"= A\n== 例子\nOne\n== 例子\nTwo"}}}));
    let diagnostic = loop {
        let message = client.receive();
        if message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == uri
        {
            break message;
        }
    };
    assert_eq!(
        diagnostic["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        diagnostic["params"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("ambiguous LabelPath")
    );
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":22}}}));
    assert!(client.response(3)["result"].is_null());
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"shutdown","params":null}));
    client.response(4);
    client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    drop(client.input.take());
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn one_server_serves_fixed_markup_and_code_frontends() {
    let mut client = Client::new();
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    client.response(1);
    client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    for (suffix, language) in [("not", "notist"), ("notc", "notist-code")] {
        let uri = notist_analysis::file_uri(&client._root.path().join(format!("example.{suffix}")));
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language,"version":1,"text":"let value = 1;"}}}));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":uri}}}));
        let symbols = client.response(2)["result"].clone();
        if suffix == "not" {
            assert_eq!(symbols, json!([]));
            client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"#let value = 1;"}]}}));
            client.send(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":uri}}}));
            assert_eq!(client.response(3)["result"][0]["name"], "value");
        } else {
            assert_eq!(symbols[0]["name"], "value");
        }
    }
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"shutdown","params":null}));
    client.response(4);
    client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    drop(client.input.take());
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn hover_references_and_rename_use_exact_ranges() {
    let mut client = Client::new();
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}}}}}));
    let capabilities = client.response(1)["result"]["capabilities"].clone();
    assert_eq!(capabilities["referencesProvider"], true);
    assert_eq!(capabilities["renameProvider"]["prepareProvider"], true);
    client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    let uri = "file:///tmp/notist-lsp-navigation.notc";
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist-code","version":7,"text":"let value = \"😀\";\nvalue;\n\"value\";"}}}));
    let pos = json!({"line":1,"character":2});
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":pos}}));
    let hover = client.response(2)["result"].clone();
    assert_eq!(hover["contents"]["kind"], "plaintext");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("let value: String")
    );
    assert_eq!(
        hover["range"],
        json!({"start":{"line":1,"character":0},"end":{"line":1,"character":5}})
    );
    for (id, include, count) in [(3, false, 1), (4, true, 2)] {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"textDocument/references","params":{"textDocument":{"uri":uri},"position":pos,"context":{"includeDeclaration":include}}}));
        assert_eq!(
            client.response(id)["result"].as_array().unwrap().len(),
            count
        );
    }
    client.send(json!({"jsonrpc":"2.0","id":5,"method":"textDocument/prepareRename","params":{"textDocument":{"uri":uri},"position":pos}}));
    assert_eq!(client.response(5)["result"]["placeholder"], "value");
    client.send(json!({"jsonrpc":"2.0","id":6,"method":"textDocument/rename","params":{"textDocument":{"uri":uri},"position":pos,"newName":"renamed"}}));
    let changes = client.response(6)["result"]["documentChanges"].clone();
    assert_eq!(changes[0]["textDocument"]["version"], 7);
    assert_eq!(changes[0]["edits"].as_array().unwrap().len(), 2);
    assert!(
        changes[0]["edits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edit| edit["range"]
                == json!({"start":{"line":0,"character":4},"end":{"line":0,"character":9}})
                && edit["newText"] == "renamed")
    );
    client.send(json!({"jsonrpc":"2.0","id":7,"method":"textDocument/rename","params":{"textDocument":{"uri":uri},"position":pos,"newName":"let"}}));
    assert_eq!(client.response(7)["error"]["code"], -32602);
    client.send(json!({"jsonrpc":"2.0","id":8,"method":"shutdown","params":null}));
    client.response(8);
    client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    drop(client.input.take());
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn rename_supports_clients_without_document_changes() {
    let mut client = Client::new();
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    client.response(1);
    client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
    let uri = "file:///tmp/notist-lsp-rename-fallback.notc";
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist-code","version":1,"text":"let value = 1;\nvalue;"}}}));
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":2},"newName":"renamed"}}));
    let result = client.response(2)["result"].clone();
    assert!(result.get("documentChanges").is_none());
    assert_eq!(result["changes"][uri].as_array().unwrap().len(), 2);
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}));
    client.response(3);
    client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    drop(client.input.take());
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn inlay_hints_and_type_annotation_diagnostics_over_stdio() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::new();
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}));
        assert_eq!(
            client.response(1)["result"]["capabilities"]["inlayHintProvider"],
            true
        );
        client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let uri = "file:///tmp/notist-inlay.notc";
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist-code","version":1,"text":"let 文 = 1;\nlet bad: Int = \"no\";\nlet f = () -> Int => 1;"}}}));
        let diagnostics = client.receive();
        assert_eq!(
            diagnostics["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}}}}));
        let hints = client.response(2)["result"].clone();
        assert_eq!(hints.as_array().unwrap().len(), 1);
        assert_eq!(hints[0]["label"], ": Int");
        assert_eq!(hints[0]["kind"], 1);
        assert_eq!(
            hints[0]["position"]["character"],
            if encoding == "utf-8" { 7 } else { 5 }
        );
        client.send(json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}));
        client.response(3);
        client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        drop(client.input.take());
        assert!(client.child.wait().unwrap().success());
    }
}

#[test]
fn stdio_unicode_edits_diagnostics_queries_and_shutdown() {
    for (encoding, end) in [("utf-8", 16), ("utf-16", 14)] {
        let mut client = Client::new();
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}));
        assert_eq!(
            client.receive()["result"]["capabilities"]["positionEncoding"],
            encoding
        );
        client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let uri = "file:///tmp/notist-lsp-regression.notc";
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist-code","version":1,"text":"let word = \"😀\";\nword;"}}}));
        assert_eq!(client.receive()["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":end}},"text":"hello"},{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}},"text":"word"}]}}));
        assert_eq!(client.receive()["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":2}}}));
        assert_eq!(client.receive()["result"]["uri"], uri);
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"let broken = ;"}]}}));
        assert!(
            !client.receive()["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}));
        assert_eq!(client.receive()["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}));
        assert_eq!(client.receive()["id"], 3);
        client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        // Closing stdin also lets the transport's reader thread finish.
        drop(client.input.take());
        assert!(client.child.wait().unwrap().success());
    }
}

#[test]
fn typed_link_diagnostics_use_negotiated_unicode_ranges_and_clear_after_fix() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::new();
        let root = client._root.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("Notist.toml"), "[package]\nname = 'unicode'\n").unwrap();
        let uri = notist_analysis::file_uri(&root.join("docs/README.not"));
        let bad = "中文😀 [[self::\"Missing\"]]";
        std::fs::write(root.join("docs/README.not"), bad).unwrap();
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}));
        client.response(1);
        client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let notification = client.receive();
        let d = &notification["params"]["diagnostics"][0];
        assert_eq!(d["code"], "missing_label");
        assert_eq!(
            d["range"]["start"]["character"],
            if encoding == "utf-8" { 11 } else { 5 }
        );
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"languageId":"notist","text":"= Missing\n中文😀 [[self::\"Missing\"]]"}}}));
        let notification = client.receive();
        assert_eq!(notification["params"]["uri"], uri);
        assert_eq!(notification["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}));
        client.response(2);
        client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        drop(client.input.take());
        assert!(client.child.wait().unwrap().success());
    }
}

#[test]
fn invalid_label_diagnostics_and_attribute_completions_use_stdio() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::new();
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}));
        client.response(1);
        client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let uri = "file:///tmp/notist-label-attributes.not";
        let bad = "@(label: 3)\n= 中文😀";
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist","version":1,"text":bad}}}));
        let notification = client.receive();
        let d = &notification["params"]["diagnostics"][0];
        assert_eq!(d["code"], "invalid_label");
        assert_eq!(
            d["range"],
            notist_analysis::range(bad, bad.find('=').unwrap(), bad.len(), encoding == "utf-8")
        );
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"@(label: \"ok\")\n= 中文😀"}]}}));
        assert_eq!(client.receive()["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":1}}}));
        let result = client.response(2);
        let candidate = result["result"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == "with_attributes")
            .unwrap();
        assert_eq!(
            candidate["detail"],
            "with_attributes(item: Content, attributes: Dict) -> Content"
        );
        client.send(json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}));
        client.response(3);
        client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        drop(client.input.take());
        assert!(client.child.wait().unwrap().success());
    }
}

#[test]
fn formation_constraints_are_published_and_cleared_in_both_encodings() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::new();
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}));
        client.response(1);
        client.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let uri = "file:///tmp/notist-formation.not";
        let bad = "#item(\"paragraph\", (body: [中文😀 #item(\"unknown\", (:))]))";
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"notist","version":1,"text":bad}}}));
        let notification = client.receive();
        let diagnostic = notification["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "content_constraint")
            .unwrap();
        let start = bad.find("item(\"unknown\"").unwrap();
        let end = bad.find("]))").unwrap();
        assert_eq!(
            diagnostic["range"],
            notist_analysis::range(bad, start, end, encoding == "utf-8")
        );
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"#item(\"paragraph\", (body: [中文😀 valid]))"}]}}));
        assert_eq!(client.receive()["params"]["diagnostics"], json!([]));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}));
        client.response(2);
        client.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        drop(client.input.take());
        assert!(client.child.wait().unwrap().success());
    }
}
