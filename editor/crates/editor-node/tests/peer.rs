use notist_editor_node::{document::*, *};
use std::sync::{Arc, Mutex};

const KEY: &str = "document-test-secret-at-least-24";
fn seed() -> SyncPacket {
    Document::new(
        DocumentIdentity {
            document_id: "note".into(),
            history_id: "shared".into(),
        },
        Some(99),
        "Hello 🧠",
    )
    .unwrap()
    .export_snapshot()
    .unwrap()
}
fn node(id: usize, seed: &SyncPacket) -> PeerNode {
    let mut node = PeerNode::new(format!("node-{id}"), format!("session-{id}")).unwrap();
    node.attach(
        Arc::new(Mutex::new(
            Document::from_snapshot(seed, Some(id as u64 + 1)).unwrap(),
        )),
        KEY.into(),
        None,
    )
    .unwrap();
    node
}
fn connect(nodes: &mut [PeerNode], a: usize, b: usize) {
    nodes[a].connect(b.to_string()).unwrap();
    nodes[b].connect(a.to_string()).unwrap();
}
fn drain(nodes: &mut [PeerNode]) {
    for _ in 0..1000 {
        let mut effects = Vec::new();
        for (index, node) in nodes.iter_mut().enumerate() {
            node.poll().unwrap();
            effects.extend(node.take_effects().into_iter().map(|e| (index, e)));
        }
        if effects.is_empty() {
            return;
        }
        for (source, effect) in effects {
            match effect {
                Effect::Send { link, message } => nodes[link.parse::<usize>().unwrap()]
                    .receive(&source.to_string(), message)
                    .unwrap(),
                Effect::Persist { document, entry } => {
                    nodes[source].persisted(&document, entry.sequence).unwrap()
                }
            }
        }
    }
    panic!("protocol did not quiesce");
}
fn append(node: &PeerNode, text: &str) {
    let handle = node.document("note").unwrap();
    let mut doc = handle.lock().unwrap();
    let before = doc.snapshot();
    doc.transact(Transaction {
        expected_version: before.version,
        origin: "test".into(),
        edits: vec![TextEdit {
            from: before.text.encode_utf16().count(),
            to: before.text.encode_utf16().count(),
            insert: text.into(),
        }],
        undo_metadata: None,
        undo_positions: vec![],
    })
    .unwrap();
}
fn text(node: &PeerNode) -> String {
    node.document("note")
        .unwrap()
        .lock()
        .unwrap()
        .snapshot()
        .text
}
fn equal(nodes: &[PeerNode]) {
    let expected = nodes[0].document("note").unwrap().lock().unwrap().version();
    for n in nodes {
        assert_eq!(
            n.document("note").unwrap().lock().unwrap().version(),
            expected
        );
    }
}

#[test]
fn chain_forwards_imports_and_triangle_quiesces() {
    let seed = seed();
    let mut nodes: Vec<_> = (0..3).map(|i| node(i, &seed)).collect();
    connect(&mut nodes, 0, 1);
    connect(&mut nodes, 1, 2);
    drain(&mut nodes);
    append(&nodes[0], " A");
    append(&nodes[2], " C");
    drain(&mut nodes);
    equal(&nodes);
    assert!(text(&nodes[1]).contains(" A"));
    assert!(text(&nodes[1]).contains(" C"));
    connect(&mut nodes, 0, 2);
    drain(&mut nodes);
    nodes[0]
        .document("note")
        .unwrap()
        .lock()
        .unwrap()
        .undo(None)
        .unwrap();
    drain(&mut nodes);
    equal(&nodes);
    assert!(!text(&nodes[1]).contains(" A"));
    assert!(text(&nodes[1]).contains(" C"));
    assert!(nodes[0].remote_durable_version("1", "note").is_some());
}
#[test]
fn partition_reconnect_and_chunked_updates() {
    let seed = seed();
    let mut nodes: Vec<_> = (0..2).map(|i| node(i, &seed)).collect();
    connect(&mut nodes, 0, 1);
    drain(&mut nodes);
    nodes[0].disconnect("1");
    nodes[1].disconnect("0");
    append(&nodes[0], &"世界🐈".repeat(10000));
    append(&nodes[1], " offline");
    drain(&mut nodes);
    assert_ne!(text(&nodes[0]), text(&nodes[1]));
    connect(&mut nodes, 0, 1);
    drain(&mut nodes);
    equal(&nodes);
    assert!(text(&nodes[0]).contains(" offline"));
}
#[test]
fn late_document_attachment_opens_both_directions() {
    let seed = seed();
    let mut nodes = vec![
        node(0, &seed),
        PeerNode::new("late".into(), "session".into()).unwrap(),
    ];
    connect(&mut nodes, 0, 1);
    drain(&mut nodes);
    append(&nodes[0], " before open");
    drain(&mut nodes);
    nodes[1]
        .attach(
            Arc::new(Mutex::new(Document::from_snapshot(&seed, Some(3)).unwrap())),
            KEY.into(),
            None,
        )
        .unwrap();
    drain(&mut nodes);
    equal(&nodes);
}
#[test]
fn durable_receipts_cover_only_the_committed_batch() {
    let mut node = node(0, &seed());
    let initial = match node.take_effects().pop().unwrap() {
        Effect::Persist { entry, .. } => entry,
        _ => unreachable!(),
    };
    append(&node, " A");
    node.poll().unwrap();
    let first = match node.take_effects().pop().unwrap() {
        Effect::Persist { entry, .. } => entry,
        _ => unreachable!(),
    };
    append(&node, " B");
    node.poll().unwrap();
    let second = match node.take_effects().pop().unwrap() {
        Effect::Persist { entry, .. } => entry,
        _ => unreachable!(),
    };
    assert!(node.persisted("note", first.sequence).is_err());
    node.persisted("note", initial.sequence).unwrap();
    node.persisted("note", first.sequence).unwrap();
    assert_eq!(node.durable_version("note"), Some(&first.applied));
    assert_ne!(node.durable_version("note"), Some(&second.applied));
    node.persisted("note", second.sequence).unwrap();
    assert_eq!(node.durable_version("note"), Some(&second.applied));
}
#[test]
fn stale_sessions_and_unauthorized_frames_are_rejected() {
    let mut node = node(0, &seed());
    node.connect("peer".into()).unwrap();
    node.receive(
        "peer",
        WireMessage {
            session: "remote".into(),
            body: Message::Hello {
                protocol: "notist-peer".into(),
                node: "remote".into(),
            },
        },
    )
    .unwrap();
    let version = node.document("note").unwrap().lock().unwrap().version();
    assert!(
        node.receive(
            "peer",
            WireMessage {
                session: "old".into(),
                body: Message::State {
                    applied: version.clone(),
                    durable: None
                }
            }
        )
        .is_err()
    );
    assert!(
        node.receive(
            "peer",
            WireMessage {
                session: "remote".into(),
                body: Message::Want {
                    version,
                    request: 1
                }
            }
        )
        .is_err()
    );
    assert!(
        node.receive(
            "peer",
            WireMessage {
                session: "remote".into(),
                body: Message::Open {
                    identity: seed().identity,
                    proof: "wrong".into()
                }
            }
        )
        .is_err()
    );
}

#[cfg(feature = "native")]
#[test]
fn journal_recovers_pending_packets_and_never_reuses_a_writer() {
    use notist_editor_node::native::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.redb");
    let seed = seed();
    let mut source = node(0, &seed);
    let mut target = node(1, &seed);
    let store = Store::open(&path).unwrap();
    for effect in target.take_effects() {
        if let Effect::Persist { document, entry } = effect {
            store.commit(&document, &entry).unwrap();
            target.persisted(&document, entry.sequence).unwrap();
        }
    }
    let initial = source.document("note").unwrap().lock().unwrap().version();
    append(&source, " A");
    let middle = source.document("note").unwrap().lock().unwrap().version();
    let a = source
        .document("note")
        .unwrap()
        .lock()
        .unwrap()
        .export_updates_since(&initial)
        .unwrap();
    append(&source, " B");
    let b = source
        .document("note")
        .unwrap()
        .lock()
        .unwrap()
        .export_updates_since(&middle)
        .unwrap();
    assert!(target.import(b, "out-of-order".into()).unwrap().pending);
    for effect in target.take_effects() {
        if let Effect::Persist { document, entry } = effect {
            store.commit(&document, &entry).unwrap();
            target.persisted(&document, entry.sequence).unwrap();
        }
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    let (restored, seq) = store.recover("note").unwrap().unwrap();
    assert_ne!(
        restored.writer_id(),
        target.document("note").unwrap().lock().unwrap().writer_id()
    );
    let mut recovered = PeerNode::new("recovered".into(), "new-session".into()).unwrap();
    recovered
        .attach(
            Arc::new(Mutex::new(restored)),
            KEY.into(),
            Some((seq, true)),
        )
        .unwrap();
    recovered.import(a, "dependency".into()).unwrap();
    assert_eq!(text(&recovered), text(&source));
    for effect in recovered.take_effects() {
        if let Effect::Persist { document, entry } = effect {
            store.commit(&document, &entry).unwrap();
            recovered.persisted(&document, entry.sequence).unwrap();
        }
    }
    drop(store);
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .recover("note")
            .unwrap()
            .unwrap()
            .0
            .snapshot()
            .text,
        text(&source)
    );
    source.poll().unwrap();
}

#[test]
fn volatile_storage_never_claims_durability() {
    let mut node = node(0, &seed());
    for effect in node.take_effects() {
        if let Effect::Persist { document, entry } = effect {
            node.stored(&document, entry.sequence, false).unwrap();
        }
    }
    assert!(node.durable_version("note").is_none());
    append(&node, " volatile");
    node.poll().unwrap();
    for effect in node.take_effects() {
        if let Effect::Persist { document, entry } = effect {
            node.stored(&document, entry.sequence, false).unwrap();
        }
    }
    assert!(node.durable_version("note").is_none());
}

#[test]
fn document_proofs_hide_credentials_and_bind_both_sessions() {
    let mut sender = node(0, &seed());
    sender.connect("peer".into()).unwrap();
    sender
        .receive(
            "peer",
            WireMessage {
                session: "recipient-session".into(),
                body: Message::Hello {
                    protocol: "notist-peer".into(),
                    node: "recipient".into(),
                },
            },
        )
        .unwrap();
    let proof = sender
        .take_effects()
        .into_iter()
        .find_map(|e| match e {
            Effect::Send {
                message:
                    m @ WireMessage {
                        body: Message::Open { .. },
                        ..
                    },
                ..
            } => Some(m),
            _ => None,
        })
        .unwrap();
    assert!(!serde_json::to_string(&proof).unwrap().contains(KEY));
    let mut recipient = PeerNode::new("recipient".into(), "different-session".into()).unwrap();
    recipient
        .attach(
            Arc::new(Mutex::new(
                Document::from_snapshot(&seed(), Some(7)).unwrap(),
            )),
            KEY.into(),
            None,
        )
        .unwrap();
    recipient.connect("sender".into()).unwrap();
    recipient
        .receive(
            "sender",
            WireMessage {
                session: "session-0".into(),
                body: Message::Hello {
                    protocol: "notist-peer".into(),
                    node: "node-0".into(),
                },
            },
        )
        .unwrap();
    assert!(recipient.receive("sender", proof).is_err());
}

#[test]
fn saturated_storage_rejects_import_before_changing_document() {
    let seed = seed();
    let mut receiver = node(0, &seed);
    let source = node(1, &seed);
    for _ in 0..1023 {
        append(&receiver, "x");
        receiver.poll().unwrap();
        receiver.take_effects();
    }
    let before = receiver.document("note").unwrap().lock().unwrap().version();
    append(&source, "remote");
    let packet = source
        .document("note")
        .unwrap()
        .lock()
        .unwrap()
        .export_snapshot()
        .unwrap();
    assert!(receiver.import(packet, "remote".into()).is_err());
    assert_eq!(
        before,
        receiver.document("note").unwrap().lock().unwrap().version()
    );
}
