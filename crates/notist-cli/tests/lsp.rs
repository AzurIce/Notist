use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::Duration,
};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
}
impl Client {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_notist"))
            .arg("lsp")
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
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
