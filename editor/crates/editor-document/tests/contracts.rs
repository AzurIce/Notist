use notist_editor_document::*;
use serde_json::json;

fn identity() -> DocumentIdentity {
    DocumentIdentity {
        document_id: "notes/draft".into(),
        history_id: "test-history".into(),
    }
}
fn doc(peer: u64, text: &str) -> Document {
    Document::new(identity(), Some(peer), text).unwrap()
}
fn copy(doc: &Document, peer: u64) -> Document {
    Document::from_snapshot(&doc.export_snapshot().unwrap(), Some(peer)).unwrap()
}
fn edit(from: usize, to: usize, insert: &str) -> TextEdit {
    TextEdit {
        from,
        to,
        insert: insert.into(),
    }
}
fn apply(doc: &mut Document, edits: Vec<TextEdit>) -> Option<ChangeEvent> {
    doc.transact(Transaction {
        expected_version: doc.version(),
        origin: "test".into(),
        edits,
        undo_metadata: None,
        undo_positions: vec![],
    })
    .unwrap()
}
fn sync(a: &Document, b: &mut Document) -> ImportResult {
    b.import(
        &a.export_updates_since(&b.version()).unwrap(),
        "peer".into(),
    )
    .unwrap()
}
fn replay(event: &ChangeEvent) {
    let mut source = event.before.text.clone();
    for edit in event.edits.iter().rev() {
        let from = utf16_to_byte(&source, edit.from).unwrap();
        let to = utf16_to_byte(&source, edit.to).unwrap();
        source.replace_range(from..to, &edit.insert);
    }
    assert_eq!(source, event.after.text);
}

#[test]
fn binary_interop_preserves_versions_and_import_guards() {
    let mut a = doc(u64::MAX - 1, "中😀");
    let version = a.version();
    assert_eq!(
        Version::decode(identity(), &version.encode().unwrap()).unwrap(),
        version
    );
    assert!(Version::decode(identity(), &[255]).is_err());
    let raw = a.export_snapshot().unwrap().data;
    let packet = SyncPacket::from_binary(identity(), raw).unwrap();
    assert_eq!(packet.kind, PacketKind::Snapshot);
    let mut b = Document::from_snapshot(&packet, Some(2)).unwrap();
    apply(&mut a, vec![edit(0, 0, "prefix")]);
    let updates = SyncPacket::from_binary(
        identity(),
        a.export_updates_since(&b.version()).unwrap().data,
    )
    .unwrap();
    assert_eq!(updates.kind, PacketKind::Updates);
    let mut wrong = updates.clone();
    wrong.identity.history_id = "unrelated".into();
    assert!(matches!(
        b.import(&wrong, "network".into()),
        Err(CoreError::IdentityMismatch)
    ));
    b.import(&updates, "network".into()).unwrap();
    assert_eq!(a.snapshot().text, b.snapshot().text);
    assert!(SyncPacket::from_binary(identity(), vec![1, 2, 3]).is_err());
}

#[test]
fn transactions_validate_all_edits_before_mutation_and_reject_stale_versions() {
    let mut d = doc(1, "A😀中BC");
    let events = d.subscribe();
    let initial = d.snapshot();
    let invalid = vec![
        vec![edit(0, 1, "X"), edit(2, 3, "bad")], // splits surrogate
        vec![edit(0, 2, "bad")],
        vec![edit(100, 100, "bad")],
        vec![edit(3, 1, "bad")],
        vec![edit(1, 4, "bad"), edit(3, 4, "bad")],
        vec![edit(1, 1, "a"), edit(1, 1, "b")],
        vec![edit(4, 4, "a"), edit(0, 0, "b")],
    ];
    for edits in invalid {
        assert!(
            d.transact(Transaction {
                expected_version: initial.version.clone(),
                origin: "bad".into(),
                edits,
                undo_metadata: None,
                undo_positions: vec![]
            })
            .is_err()
        );
        assert_eq!(d.snapshot(), initial);
        assert!(!d.undo_state().can_undo);
        assert!(events.try_recv().is_err());
    }
    let event = apply(&mut d, vec![edit(1, 3, "文"), edit(4, 6, "🦀")]).unwrap();
    assert_eq!(event.after.text, "A文中🦀");
    replay(&event);
    assert_eq!(events.try_recv().unwrap(), event);
    assert!(matches!(
        d.transact(Transaction {
            expected_version: initial.version,
            origin: "stale".into(),
            edits: vec![edit(0, 0, "x")],
            undo_metadata: None,
            undo_positions: vec![]
        }),
        Err(CoreError::StaleVersion)
    ));
    d.undo(None).unwrap();
    assert_eq!(d.snapshot().text, "A😀中BC");
}

#[test]
fn snapshots_and_subscriptions_are_owned_ordered_and_not_reentrant() {
    let mut d = doc(1, "base");
    let first = d.subscribe();
    let second = d.subscribe();
    let saved = d.snapshot();
    assert!(apply(&mut d, vec![edit(0, 4, "base")]).is_none());
    assert!(first.try_recv().is_err());
    apply(&mut d, vec![edit(4, 4, " 中😀")]);
    apply(&mut d, vec![edit(0, 4, "text")]);
    d.undo(None).unwrap();
    d.redo(None).unwrap();
    let events: Vec<_> = first.try_iter().collect();
    assert_eq!(events, second.try_iter().collect::<Vec<_>>());
    assert_eq!(events.len(), 4);
    for (i, event) in events.iter().enumerate() {
        replay(event);
        assert_eq!(event.before.revision, i as u64);
        assert_eq!(event.after.revision, i as u64 + 1);
    }
    assert_eq!(saved.text, "base");
    assert_eq!(saved.revision, 0);
    drop(first);
    drop(second);
    apply(&mut d, vec![edit(0, 0, "still editable ")]);
}

#[test]
fn collaborative_undo_preserves_remote_text_and_restores_opaque_host_metadata() {
    let mut a = doc(1, "");
    let selections = json!({"view": "code", "ranges": [[0, 0], [0, 0]], "main": 1});
    a.transact(Transaction {
        expected_version: a.version(),
        origin: "view".into(),
        edits: vec![edit(0, 0, "abc")],
        undo_metadata: Some(selections.clone()),
        undo_positions: vec![],
    })
    .unwrap();
    let mut b = copy(&a, 2);
    apply(&mut b, vec![edit(1, 1, "中😀")]);
    sync(&b, &mut a);
    assert_eq!(a.snapshot().text, "a中😀bc");
    let undo = a
        .undo(Some(json!({"view": "rich", "cursor": 3})))
        .unwrap()
        .unwrap();
    assert_eq!(undo.after.text, "中😀");
    assert_eq!(undo.restored_metadata, Some(selections));
    replay(&undo);
    let redo = a.redo(None).unwrap().unwrap();
    assert_eq!(redo.after.text, "a中😀bc");
    assert_eq!(
        redo.restored_metadata,
        Some(json!({"view": "rich", "cursor": 3}))
    );
    sync(&a, &mut b);
    assert_eq!(a.version(), b.version());
    assert_eq!(a.snapshot().text, b.snapshot().text);
}

#[test]
fn history_changes_are_observable_even_when_remote_deletion_makes_undo_textually_empty() {
    let mut a = doc(1, "");
    apply(&mut a, vec![edit(0, 0, "local")]);
    let mut b = copy(&a, 2);
    apply(&mut b, vec![edit(0, 5, "")]);
    sync(&b, &mut a);
    let events = a.subscribe();
    assert!(a.undo_state().can_undo);
    a.undo(None)
        .unwrap()
        .expect("consuming a history entry must notify observers");
    assert_eq!(a.snapshot().text, "");
    assert!(!a.undo_state().can_undo);
    let event = events.try_recv().unwrap();
    assert!(event.edits.is_empty());
    replay(&event);
    apply(&mut a, vec![edit(0, 0, "next")]);
    events.try_iter().for_each(drop);
    a.clear_undo();
    assert!(matches!(
        events.try_recv().unwrap().cause,
        ChangeCause::HistoryCleared
    ));
    assert!(!a.undo_state().can_undo);
}

#[test]
fn undo_positions_restore_replaced_ranges_and_transform_all_endpoints_through_remote_edits() {
    let mut a = doc(1, "Hello world.");
    a.transact(Transaction {
        expected_version: a.version(),
        origin: "rich".into(),
        edits: vec![edit(6, 11, "*world*")],
        undo_metadata: Some(json!({"view":"rich", "main":1})),
        undo_positions: vec![6, 11, 0, 5],
    })
    .unwrap();
    let mut b = copy(&a, 2);
    apply(&mut b, vec![edit(0, 0, "远🧠")]);
    sync(&b, &mut a);
    let context = UndoContext {
        metadata: Some(json!({"view":"source"})),
        positions: vec![9, 16, 3, 8],
    };
    let event = a.undo_with_context(context).unwrap().unwrap();
    assert_eq!(event.after.text, "远🧠Hello world.");
    // Loro's undo-position transform stays before insertion at the exact gap.
    assert_eq!(event.restored_positions, vec![9, 14, 0, 8]);
    assert_eq!(
        event.restored_metadata,
        Some(json!({"view":"rich", "main":1}))
    );
    let redo = a
        .redo_with_context(UndoContext::default())
        .unwrap()
        .unwrap();
    assert_eq!(redo.after.text, "远🧠Hello *world*.");
    assert_eq!(redo.restored_positions, vec![9, 16, 3, 8]);
    let snapshot = a.snapshot();
    assert!(
        a.transact(Transaction {
            expected_version: a.version(),
            origin: "invalid".into(),
            edits: vec![edit(0, 0, "bad")],
            undo_metadata: None,
            undo_positions: vec![2],
        })
        .is_err()
    );
    assert_eq!(a.snapshot(), snapshot);

    let mut end = doc(10, "hello");
    end.transact(Transaction {
        expected_version: end.version(),
        origin: "test".into(),
        edits: vec![edit(0, 1, "H")],
        undo_metadata: None,
        undo_positions: vec![5],
    })
    .unwrap();
    let mut remote = copy(&end, 11);
    apply(&mut remote, vec![edit(5, 5, "🧠")]);
    sync(&remote, &mut end);
    assert_eq!(end.undo(None).unwrap().unwrap().restored_positions, vec![7]);
}

#[test]
fn undo_groups_have_explicit_boundaries_and_import_closes_them() {
    let mut a = doc(1, "");
    a.begin_undo_group().unwrap();
    assert!(matches!(
        a.begin_undo_group(),
        Err(CoreError::UndoGroupOpen)
    ));
    apply(&mut a, vec![edit(0, 0, "n")]);
    apply(&mut a, vec![edit(0, 1, "你")]);
    apply(&mut a, vec![edit(1, 1, "好")]);
    a.end_undo_group();
    a.undo(None).unwrap();
    assert_eq!(a.snapshot().text, "");
    a.redo(None).unwrap();
    assert_eq!(a.snapshot().text, "你好");
    let mut b = copy(&a, 2);
    a.begin_undo_group().unwrap();
    apply(&mut a, vec![edit(2, 2, "A")]);
    apply(&mut b, vec![edit(0, 0, "远")]);
    sync(&b, &mut a);
    assert!(!a.undo_state().group_open);
    apply(&mut a, vec![edit(4, 4, "B")]);
    a.undo(None).unwrap();
    assert_eq!(a.snapshot().text, "远你好A");
    a.undo(None).unwrap();
    assert_eq!(a.snapshot().text, "远你好");
}

#[test]
fn anchors_have_insertion_affinity_and_survive_deletion_and_snapshot_transfer() {
    let mut a = doc(1, "a😀b");
    let mut b = copy(&a, 2);
    let before = a.anchor_at(3, Affinity::Before).unwrap();
    let after = a.anchor_at(3, Affinity::After).unwrap();
    let start = a.anchor_at(0, Affinity::Before).unwrap();
    let end = a.anchor_at(4, Affinity::After).unwrap();
    assert!(a.anchor_at(2, Affinity::After).is_err());
    apply(&mut b, vec![edit(3, 3, "中")]);
    sync(&b, &mut a);
    assert_eq!(a.resolve_anchor(&before).unwrap().offset, 3);
    assert_eq!(a.resolve_anchor(&after).unwrap().offset, 4);
    apply(&mut a, vec![edit(0, 0, "前"), edit(5, 5, "后")]);
    assert_eq!(a.resolve_anchor(&start).unwrap().offset, 0);
    assert_eq!(a.resolve_anchor(&end).unwrap().offset, 7);
    apply(&mut a, vec![edit(2, 6, "")]);
    assert_eq!(a.snapshot().text, "前a后");
    assert_eq!(a.resolve_anchor(&before).unwrap().offset, 2);
    assert_eq!(a.resolve_anchor(&after).unwrap().offset, 2);
    let recovered = copy(&a, 3);
    let encoded: Anchor = serde_json::from_str(&serde_json::to_string(&after).unwrap()).unwrap();
    let resolved = recovered.resolve_anchor(&encoded).unwrap();
    assert_eq!(resolved.offset, 2);
    assert_eq!(
        recovered
            .resolve_anchor(&resolved.refreshed)
            .unwrap()
            .offset,
        2
    );
    let mut empty = doc(10, "");
    let left = empty.anchor_at(0, Affinity::Before).unwrap();
    let right = empty.anchor_at(0, Affinity::After).unwrap();
    apply(&mut empty, vec![edit(0, 0, "🧠")]);
    assert_eq!(empty.resolve_anchor(&left).unwrap().offset, 0);
    assert_eq!(empty.resolve_anchor(&right).unwrap().offset, 2);
}

#[test]
fn document_history_writer_and_packet_boundaries_are_enforced() {
    let mut a = doc(1, "abc");
    let mut b = copy(&a, 2);
    let saved = a.snapshot();
    let mut packet = b.export_snapshot().unwrap();
    packet.identity.history_id = "unrelated".into();
    assert!(matches!(
        a.import(&packet, "bad".into()),
        Err(CoreError::IdentityMismatch)
    ));
    assert!(matches!(
        Document::from_snapshot(&a.export_snapshot().unwrap(), Some(1)),
        Err(CoreError::WriterAlreadyUsed { .. })
    ));
    let mut foreign = doc(2, "");
    assert!(matches!(
        foreign.import(&b.export_snapshot().unwrap(), "bad".into()),
        Ok(_)
    )); // peer 2 has no operations in b
    apply(&mut b, vec![edit(0, 0, "new")]);
    assert!(matches!(
        foreign.import(&b.export_snapshot().unwrap(), "bad".into()),
        Err(CoreError::WriterCollision)
    ));
    packet = b.export_snapshot().unwrap();
    packet.kind = PacketKind::Updates;
    assert!(matches!(
        a.import(&packet, "bad".into()),
        Err(CoreError::InvalidPacket)
    ));
    packet.kind = PacketKind::Snapshot;
    packet.data.truncate(packet.data.len() / 2);
    assert!(a.import(&packet, "bad".into()).is_err());
    assert_eq!(a.snapshot(), saved);
    let restored = copy(&a, 3);
    assert_eq!(restored.version(), a.version());
    assert!(!restored.undo_state().can_undo);
    let other = Document::new(
        DocumentIdentity {
            document_id: "other".into(),
            ..identity()
        },
        Some(5),
        "",
    )
    .unwrap();
    assert!(matches!(
        other.resolve_anchor(&a.anchor_at(1, Affinity::Before).unwrap()),
        Err(CoreError::IdentityMismatch)
    ));
}

#[test]
fn shallow_history_and_non_text_documents_are_rejected() {
    let external = loro::LoroDoc::new();
    external.get_text("source").insert(0, "abc").unwrap();
    external.commit();
    let packet = SyncPacket {
        identity: identity(),
        kind: PacketKind::Snapshot,
        data: external
            .export(loro::ExportMode::shallow_snapshot(
                &external.state_frontiers(),
            ))
            .unwrap(),
    };
    assert!(matches!(
        Document::from_snapshot(&packet, Some(1)),
        Err(CoreError::UnsupportedHistory)
    ));
    external.get_map("view").insert("cursor", 42).unwrap();
    external.commit();
    let packet = SyncPacket {
        data: external.export(loro::ExportMode::Snapshot).unwrap(),
        ..packet
    };
    assert!(matches!(
        Document::from_snapshot(&packet, Some(1)),
        Err(CoreError::InvalidPacket)
    ));
    assert!(matches!(
        doc(4, "").import(&packet, "external".into()),
        Err(CoreError::InvalidPacket)
    ));
}

#[test]
fn causally_premature_and_duplicate_packets_do_not_lose_updates_or_emit_fake_changes() {
    let mut a = doc(1, "");
    let mut b = copy(&a, 2);
    let events = b.subscribe();
    let start = a.version();
    apply(&mut a, vec![edit(0, 0, "A")]);
    let first = a.export_updates_since(&start).unwrap();
    let middle = a.version();
    apply(&mut a, vec![edit(1, 1, "中😀")]);
    let second = a.export_updates_since(&middle).unwrap();
    let result = b.import(&second, "out-of-order".into()).unwrap();
    assert!(result.pending);
    assert!(result.event.is_none());
    assert_eq!(b.snapshot().text, "");
    assert!(events.try_recv().is_err());
    b.import(&second, "duplicate".into()).unwrap();
    b.import(&first, "predecessor".into()).unwrap();
    assert_eq!(b.snapshot().text, "A中😀");
    assert_eq!(a.version(), b.version());
    let changed: Vec<_> = events.try_iter().collect();
    assert_eq!(changed.len(), 1);
    replay(&changed[0]);
    assert!(
        b.import(&first, "duplicate".into())
            .unwrap()
            .event
            .is_none()
    );
    assert!(events.try_recv().is_err());
    assert!(!b.undo_state().can_undo);
}

#[test]
fn shuffled_offline_replicas_converge_and_recover_with_unicode() {
    for seed in 1..=16u64 {
        let base = doc(99, "= 文档\nA😀B\né\n");
        let mut replicas = [copy(&base, 1), copy(&base, 2), copy(&base, 3)];
        let mut packets = Vec::new();
        let mut random = seed;
        let mut rand = |n: usize| {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            (random as usize) % n
        };
        for replica in &mut replicas {
            let mut oracle = replica.snapshot().text;
            for _ in 0..25 {
                let before = replica.version();
                let boundaries: Vec<_> = std::iter::once(0)
                    .chain(oracle.chars().scan(0, |n, c| {
                        *n += c.len_utf16();
                        Some(*n)
                    }))
                    .collect();
                let deletion = rand(3) == 0 && boundaries.len() > 1;
                let i = rand(boundaries.len() - usize::from(deletion));
                let from = boundaries[i];
                let to = if deletion { boundaries[i + 1] } else { from };
                let insert = if deletion {
                    ""
                } else {
                    ["中", "😀", "é", "\n", "abc", "👩‍💻"][rand(6)]
                };
                let from_byte = utf16_to_byte(&oracle, from).unwrap();
                let to_byte = utf16_to_byte(&oracle, to).unwrap();
                oracle.replace_range(from_byte..to_byte, insert);
                let event = apply(replica, vec![edit(from, to, insert)]).unwrap();
                replay(&event);
                assert_eq!(replica.snapshot().text, oracle);
                packets.push(replica.export_updates_since(&before).unwrap());
            }
        }
        for i in (1..packets.len()).rev() {
            let j = rand(i + 1);
            packets.swap(i, j);
        }
        for (i, replica) in replicas.iter_mut().enumerate() {
            let iter: Box<dyn Iterator<Item = &SyncPacket>> = if i % 2 == 0 {
                Box::new(packets.iter())
            } else {
                Box::new(packets.iter().rev())
            };
            for packet in iter {
                if let Some(event) = replica.import(packet, "offline".into()).unwrap().event {
                    replay(&event);
                }
                assert!(
                    replica
                        .import(packet, "duplicate".into())
                        .unwrap()
                        .event
                        .is_none()
                );
            }
        }
        for replica in &replicas[1..] {
            assert_eq!(
                replica.snapshot().text,
                replicas[0].snapshot().text,
                "seed {seed}"
            );
            assert_eq!(replica.version(), replicas[0].version());
        }
        let mut recovered = copy(&base, 4);
        for packet in packets.iter().rev() {
            recovered.import(packet, "restore".into()).unwrap();
        }
        assert_eq!(recovered.version(), replicas[0].version());
        assert_eq!(
            copy(&recovered, 5).snapshot().text,
            replicas[0].snapshot().text
        );
    }
}
