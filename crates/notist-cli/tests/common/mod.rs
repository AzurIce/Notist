//! Shared black-box LSP test client; helpers may be unused per test binary.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufReader, Read, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::PublishDiagnosticsParams;
use lsp_types::notification::{LogMessage, Notification as _, PublishDiagnostics};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(15);

const URI_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']');

pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    format!("file://{}", utf8_percent_encode(&text, URI_ENCODE_SET))
}

pub struct Vault {
    dir: tempfile::TempDir,
}

impl Vault {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().expect("failed to create a temporary vault");
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().expect("vault file has a parent directory"))
                .expect("failed to create vault subdirectory");
            std::fs::write(path, contents).expect("failed to write a vault file");
        }
        Self { dir }
    }

    pub fn root_uri(&self) -> String {
        file_uri(&dunce::canonicalize(self.dir.path()).expect("canonical vault root"))
    }

    pub fn uri(&self, name: &str) -> String {
        file_uri(&dunce::canonicalize(self.dir.path().join(name)).expect("canonical vault file"))
    }
}

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    inbox: mpsc::Receiver<Message>,
    backlog: VecDeque<Message>,
    next_id: i32,
    stderr: Arc<Mutex<String>>,
}

impl Client {
    pub fn spawn(vault: &Vault) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_notist"))
            .args(["lsp", "--no-daemon"])
            .current_dir(vault.dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `notist lsp --no-daemon`");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut stderr = child.stderr.take().expect("child stderr");

        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        std::thread::spawn({
            let stderr_buffer = Arc::clone(&stderr_buffer);
            move || {
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut buffer) = stderr_buffer.lock() {
                                buffer.push_str(&String::from_utf8_lossy(&chunk[..read]));
                            }
                        }
                    }
                }
            }
        });

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Ok(Some(message)) = Message::read(&mut stdout) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            inbox: receiver,
            backlog: VecDeque::new(),
            next_id: 0,
            stderr: stderr_buffer,
        }
    }

    pub fn initialize(&mut self, vault: &Vault) -> Value {
        let root = vault.root_uri();
        let id = self.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root,
                "capabilities": {},
                "workspaceFolders": [{"uri": root, "name": "vault"}],
            }),
        );
        let result = ok_result(self.await_response(id));
        self.notify("initialized", serde_json::json!({}));
        result["capabilities"].clone()
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(Message::Notification(Notification::new(
            method.to_owned(),
            params,
        )));
    }

    pub fn request(&mut self, method: &str, params: Value) -> i32 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(Message::Request(Request::new(
            RequestId::from(id),
            method.to_owned(),
            params,
        )));
        id
    }

    pub fn await_response(&mut self, id: i32) -> Response {
        let wanted = RequestId::from(id);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let mut matched = None;
            for _ in 0..self.backlog.len() {
                let message = self.backlog.pop_front().expect("backlog length is stable");
                match message {
                    Message::Response(response) if response.id == wanted => {
                        matched = Some(response);
                        break;
                    }
                    message => self.backlog.push_back(message),
                }
            }
            if let Some(response) = matched {
                return response;
            }
            self.recv_within(deadline, "a response");
        }
    }

    pub fn expect_notification(
        &mut self,
        method: &str,
        predicate: impl Fn(&Notification) -> bool,
        description: &str,
    ) -> Notification {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let mut matched = None;
            for _ in 0..self.backlog.len() {
                let message = self.backlog.pop_front().expect("backlog length is stable");
                match message {
                    Message::Notification(notification)
                        if notification.method == method && predicate(&notification) =>
                    {
                        matched = Some(notification);
                        break;
                    }
                    message => self.backlog.push_back(message),
                }
            }
            if let Some(notification) = matched {
                return notification;
            }
            self.recv_within(deadline, description);
        }
    }

    pub fn expect_diagnostics(
        &mut self,
        uri: &str,
        predicate: impl Fn(&PublishDiagnosticsParams) -> bool,
        description: &str,
    ) -> PublishDiagnosticsParams {
        let notification = self.expect_notification(
            PublishDiagnostics::METHOD,
            |notification| {
                serde_json::from_value::<PublishDiagnosticsParams>(notification.params.clone())
                    .map(|params| params.uri.as_str() == uri && predicate(&params))
                    .unwrap_or(false)
            },
            description,
        );
        serde_json::from_value(notification.params).expect("publishDiagnostics params")
    }

    pub fn expect_log_message(&mut self, needle: &str) -> Value {
        self.expect_notification(
            LogMessage::METHOD,
            |notification| {
                notification.params["type"] == 2
                    && notification.params["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(needle))
            },
            &format!("window/logMessage warning containing `{needle}`"),
        )
        .params
    }

    pub fn wait_for_stderr(&self, needle: &str) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.stderr().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    pub fn shutdown_and_exit(mut self) -> std::process::ExitStatus {
        let id = self.request("shutdown", Value::Null);
        let response = self.await_response(id);
        assert!(
            response.response_result.is_ok(),
            "shutdown returned an error"
        );
        self.notify("exit", Value::Null);
        drop(self.stdin.take());
        self.child.wait().expect("failed to reap notist lsp")
    }

    fn send(&mut self, message: Message) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("client stdin closed while the session is live");
        message
            .write(stdin)
            .expect("failed to write an LSP frame");
        stdin.flush().expect("failed to flush an LSP frame");
    }

    fn recv_within(&mut self, deadline: Instant, waiting_for: &str) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out after {TIMEOUT:?} waiting for {waiting_for}");
        }
        match self.inbox.recv_timeout(remaining) {
            Ok(message) => self.backlog.push_back(message),
            Err(_) => panic!("timed out after {TIMEOUT:?} waiting for {waiting_for}"),
        }
    }

    fn stderr(&self) -> String {
        self.stderr.lock().expect("stderr buffer poisoned").clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn ok_result(response: Response) -> Value {
    match response.response_result {
        Ok(result) => result,
        Err(error) => panic!("LSP request failed {}: {}", error.code, error.message),
    }
}
