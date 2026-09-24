#![cfg(feature = "native")]
use notist_editor_node::{
    document::*,
    native::{NodeRuntime, TextFileState, config::*},
};
use std::{fs, net::TcpListener, path::Path, time::Duration};

fn config(dir: &std::path::Path, peer: bool, network: bool) -> Config {
    Config {
        state_dir: dir.to_owned(),
        access_token: "test-node-access-token-123456".into(),
        http: HttpConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            public_url: "http://127.0.0.1".into(),
            tls: None,
        },
        peer: PeerConfig {
            enabled: peer,
            documents: if peer {
                vec![DocumentConfig {
                    identity: DocumentIdentity {
                        document_id: "note".into(),
                        history_id: "shared".into(),
                    },
                    credential: "test-document-credential-123456".into(),
                    initial_text: String::new(),
                    text_file: None,
                }]
            } else {
                vec![]
            },
            ..Default::default()
        },
        network_service: NetworkServiceConfig {
            enabled: network,
            turn: network.then(|| TurnConfig {
                listen_udp: "127.0.0.1:0".parse().unwrap(),
                listen_tcp: "127.0.0.1:0".parse().unwrap(),
                listen_tls: None,
                public_ip: "127.0.0.1".parse().unwrap(),
                public_host: "127.0.0.1".into(),
                relay_ports: [53000, 53100],
            }),
        },
    }
}
async fn stop(runtime: NodeRuntime) {
    tokio::time::timeout(Duration::from_secs(15), runtime.shutdown())
        .await
        .unwrap()
        .unwrap();
}

fn replace(runtime: &NodeRuntime, text: &str) -> Version {
    let doc = runtime
        .peer()
        .unwrap()
        .lock()
        .unwrap()
        .document("note")
        .unwrap();
    let mut doc = doc.lock().unwrap();
    let snapshot = doc.snapshot();
    doc.transact(Transaction {
        expected_version: snapshot.version,
        origin: "test".into(),
        edits: vec![TextEdit {
            from: 0,
            to: snapshot.text.encode_utf16().count(),
            insert: text.into(),
        }],
        undo_metadata: None,
        undo_positions: vec![],
    })
    .unwrap();
    doc.version()
}

async fn wait_file(runtime: &NodeRuntime, path: &Path, text: &str, version: &Version) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = runtime.text_files().into_iter().next().unwrap();
            if status.state == TextFileState::Synced && status.version.as_ref() == Some(version) {
                assert_eq!(fs::read_to_string(path).unwrap(), text);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn services_bind_atomically_and_relay_only_never_opens_document_store() {
    let dir = tempfile::tempdir().unwrap();
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let mut conf = config(dir.path(), false, true);
    conf.http.listen = address;
    conf.network_service.turn.as_mut().unwrap().listen_tcp = occupied.local_addr().unwrap();
    assert!(NodeRuntime::start(conf).await.is_err());
    let released = TcpListener::bind(address).unwrap();
    drop(released);
    let runtime = NodeRuntime::start(config(dir.path(), false, true))
        .await
        .unwrap();
    let address = runtime.address;
    assert!(runtime.peer().is_none());
    assert!(!dir.path().join("documents.redb").exists());
    stop(runtime).await;
    let _released = TcpListener::bind(address).unwrap();
}

#[tokio::test]
async fn native_websocket_chain_forwards_changes_and_flushes_on_shutdown() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = NodeRuntime::start(config(dirs[0].path(), true, false))
        .await
        .unwrap();
    let mut bc = config(dirs[1].path(), true, false);
    bc.peer.connect.push(RemoteConfig {
        url: format!("ws://{}/peer", a.address),
        access_token: bc.access_token.clone(),
    });
    let b = NodeRuntime::start(bc).await.unwrap();
    let mut cc = config(dirs[2].path(), true, false);
    let text_file = dirs[2].path().join("source/note.not");
    cc.peer.documents[0].text_file = Some(text_file.clone());
    cc.peer.connect.push(RemoteConfig {
        url: format!("ws://{}/peer", b.address),
        access_token: cc.access_token.clone(),
    });
    let c = NodeRuntime::start(cc.clone()).await.unwrap();
    let node = a.peer().unwrap();
    let doc = node.lock().unwrap().document("note").unwrap();
    {
        let mut doc = doc.lock().unwrap();
        let version = doc.version();
        doc.transact(Transaction {
            expected_version: version,
            origin: "native-view".into(),
            edits: vec![TextEdit {
                from: 0,
                to: 0,
                insert: "native chain 🧠".into(),
            }],
            undo_metadata: None,
            undo_positions: vec![],
        })
        .unwrap();
    }
    let expected = doc.lock().unwrap().version();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if c.peer().unwrap().lock().unwrap().durable_version("note") == Some(&expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    wait_file(&c, &text_file, "native chain 🧠", &expected).await;
    assert!(a.text_files().is_empty());
    stop(c).await;
    stop(b).await;
    stop(a).await;
    fs::remove_file(&text_file).unwrap();
    let restored = NodeRuntime::start(cc).await.unwrap();
    assert_eq!(
        restored
            .peer()
            .unwrap()
            .lock()
            .unwrap()
            .document("note")
            .unwrap()
            .lock()
            .unwrap()
            .snapshot()
            .text,
        "native chain 🧠"
    );
    assert_eq!(fs::read_to_string(&text_file).unwrap(), "native chain 🧠");
    stop(restored).await;
}

#[tokio::test]
async fn text_files_follow_edit_undo_redo_and_shutdown_without_normalizing_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source/note.not");
    let mut conf = config(dir.path(), true, false);
    conf.peer.documents[0].text_file = Some(path.clone());
    conf.peer.documents[0].initial_text = "初始 🧠\r\n\n".into();
    let runtime = NodeRuntime::start(conf.clone()).await.unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        conf.peer.documents[0].initial_text
    );
    let version = replace(&runtime, "编辑 👨‍👩‍👧\r\n\n最后没有换行");
    wait_file(&runtime, &path, "编辑 👨‍👩‍👧\r\n\n最后没有换行", &version).await;
    let doc = runtime
        .peer()
        .unwrap()
        .lock()
        .unwrap()
        .document("note")
        .unwrap();
    let version = {
        let mut doc = doc.lock().unwrap();
        doc.undo(None).unwrap();
        doc.version()
    };
    wait_file(
        &runtime,
        &path,
        &conf.peer.documents[0].initial_text,
        &version,
    )
    .await;
    let version = {
        let mut doc = doc.lock().unwrap();
        doc.redo(None).unwrap();
        doc.version()
    };
    wait_file(&runtime, &path, "编辑 👨‍👩‍👧\r\n\n最后没有换行", &version).await;
    // No wait between edit and shutdown: both journal and text must be drained.
    replace(&runtime, "");
    stop(runtime).await;
    assert_eq!(fs::read(&path).unwrap(), b"");
    let restored = NodeRuntime::start(conf).await.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"");
    stop(restored).await;
}

#[tokio::test]
async fn external_changes_are_preserved_without_import_and_output_can_resume() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.not");
    fs::write(&path, "external file").unwrap();
    let mut conf = config(dir.path(), true, false);
    conf.peer.documents[0].text_file = Some(path.clone());
    conf.peer.documents[0].initial_text = "CRDT".into();
    let runtime = NodeRuntime::start(conf.clone()).await.unwrap();
    assert_eq!(runtime.text_files()[0].state, TextFileState::Conflict);
    assert_eq!(fs::read_to_string(&path).unwrap(), "external file");
    let version = replace(&runtime, "CRDT edit");
    fs::rename(&path, dir.path().join("external-backup.not")).unwrap();
    wait_file(&runtime, &path, "CRDT edit", &version).await;
    fs::write(&path, "external edit").unwrap();
    let version = replace(&runtime, "second CRDT edit");
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.text_files()[0].state != TextFileState::Conflict {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
    assert_eq!(
        runtime
            .peer()
            .unwrap()
            .lock()
            .unwrap()
            .durable_version("note"),
        Some(&version)
    );
    stop(runtime).await;
    let restored = NodeRuntime::start(conf).await.unwrap();
    assert_eq!(restored.text_files()[0].state, TextFileState::Conflict);
    assert_eq!(
        restored
            .peer()
            .unwrap()
            .lock()
            .unwrap()
            .document("note")
            .unwrap()
            .lock()
            .unwrap()
            .snapshot()
            .text,
        "second CRDT edit"
    );
    fs::remove_file(&path).unwrap();
    // A final flush bypasses the conflict retry delay.
    stop(restored).await;
    assert_eq!(fs::read_to_string(&path).unwrap(), "second CRDT edit");
    assert_eq!(
        fs::read_to_string(dir.path().join("external-backup.not")).unwrap(),
        "external file"
    );
}

#[tokio::test]
async fn text_output_error_does_not_block_journal_and_retries_after_repair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.not");
    fs::create_dir(&path).unwrap();
    let mut conf = config(dir.path(), true, false);
    conf.peer.documents[0].text_file = Some(path.clone());
    let runtime = NodeRuntime::start(conf).await.unwrap();
    assert_eq!(runtime.text_files()[0].state, TextFileState::Error);
    let version = replace(&runtime, "still durable 🧠");
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime
            .peer()
            .unwrap()
            .lock()
            .unwrap()
            .durable_version("note")
            != Some(&version)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();
    wait_file(&runtime, &path, "still durable 🧠", &version).await;
    stop(runtime).await;
}

#[tokio::test]
async fn text_output_paths_are_config_relative_and_cannot_alias_each_other_or_storage() {
    let dir = tempfile::tempdir().unwrap();
    let mut conf = config(dir.path(), true, false);
    conf.peer.documents[0].text_file = Some("sources/note.not".into());
    let config_file = dir.path().join("node.toml");
    fs::write(&config_file, toml::to_string(&conf).unwrap()).unwrap();
    let mut conf = Config::read(&config_file).unwrap();
    assert_eq!(
        conf.peer.documents[0].text_file.as_deref(),
        Some(dir.path().join("sources/note.not").as_path())
    );
    let runtime = NodeRuntime::start(conf.clone()).await.unwrap();
    stop(runtime).await;
    let mut other = conf.peer.documents[0].clone();
    other.identity.document_id = "other".into();
    other.text_file = Some(dir.path().join("sources/../sources/note.not"));
    conf.peer.documents.push(other);
    assert!(
        NodeRuntime::start(conf.clone())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("share a text_file")
    );
    conf.peer.documents.pop();
    conf.peer.documents[0].text_file = Some(dir.path().join("documents.redb"));
    assert!(
        NodeRuntime::start(conf)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("cannot overwrite node storage")
    );
}
