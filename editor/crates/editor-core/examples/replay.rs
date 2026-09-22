//! Native side of the native/Wasm contract replay. No language or UI dependency.
use notist_editor_core::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Read},
};

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let request: Value = serde_json::from_str(&input).unwrap();
    if let Some(packet) = request.get("restore") {
        let mut restored =
            Document::from_snapshot(&serde_json::from_value(packet.clone()).unwrap(), Some(901))
                .unwrap();
        let imported = restored.snapshot();
        restored
            .transact(Transaction {
                expected_version: restored.version(),
                origin: "native".into(),
                edits: vec![TextEdit {
                    from: imported.text.encode_utf16().count(),
                    to: imported.text.encode_utf16().count(),
                    insert: "\n来自 Rust 🦀".into(),
                }],
                undo_metadata: None,
                undo_positions: vec![],
            })
            .unwrap();
        println!(
            "{}",
            json!({"imported":imported,"snapshot":restored.snapshot(),"packet":restored.export_snapshot().unwrap()})
        );
        return;
    }
    let base = Document::new(
        DocumentIdentity {
            document_id: "replay".into(),
            history_id: "shared-history".into(),
        },
        Some(99),
        request["initial"].as_str().unwrap(),
    )
    .unwrap();
    let mut docs: Vec<_> = (1..=3)
        .map(|peer| Document::from_snapshot(&base.export_snapshot().unwrap(), Some(peer)).unwrap())
        .collect();
    let mut packets = Vec::new();
    let mut anchors = HashMap::new();
    let mut trace = Vec::new();
    for step in request["steps"].as_array().unwrap() {
        let peer = step["peer"].as_u64().unwrap() as usize;
        let doc = &mut docs[peer];
        let mut observed = Value::Null;
        match step["op"].as_str().unwrap() {
            "edit" => {
                let before = doc.version();
                doc.transact(Transaction {
                    expected_version: before.clone(),
                    origin: "replay".into(),
                    edits: serde_json::from_value(step["edits"].clone()).unwrap(),
                    undo_metadata: None,
                    undo_positions: vec![],
                })
                .unwrap();
                packets.push(doc.export_updates_since(&before).unwrap());
            }
            "deliver" => {
                observed = json!(
                    doc.import(
                        &packets[step["packet"].as_u64().unwrap() as usize],
                        "replay".into()
                    )
                    .unwrap()
                    .pending
                );
            }
            "undo" | "redo" => {
                let before = doc.version();
                observed = json!(
                    if step["op"] == "undo" {
                        doc.undo(None)
                    } else {
                        doc.redo(None)
                    }
                    .unwrap()
                    .is_some()
                );
                packets.push(doc.export_updates_since(&before).unwrap());
            }
            "begin" => doc.begin_undo_group().unwrap(),
            "end" => doc.end_undo_group(),
            "anchor" => {
                anchors.insert(
                    step["name"].as_str().unwrap().to_owned(),
                    doc.anchor_at(
                        step["offset"].as_u64().unwrap() as usize,
                        serde_json::from_value(step["affinity"].clone()).unwrap(),
                    )
                    .unwrap(),
                );
            }
            "resolve" => {
                observed = json!(
                    doc.resolve_anchor(&anchors[step["name"].as_str().unwrap()])
                        .unwrap()
                        .offset
                );
            }
            other => panic!("unknown replay operation {other}"),
        }
        trace.push(json!({"snapshots":docs.iter().map(Document::snapshot).collect::<Vec<_>>(),"undo":docs.iter().map(Document::undo_state).collect::<Vec<_>>(),"observed":observed}));
    }
    println!(
        "{}",
        json!({"trace":trace,"packet":docs[0].export_snapshot().unwrap()})
    );
}
