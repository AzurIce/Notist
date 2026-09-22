use loro::cursor::{Cursor, Side};
use loro::{ExportMode, LoroDoc, UndoManager};
use serde_json::{Value, json};
use std::{collections::HashMap, env, fs};

fn utf16_to_unicode(text: &str, position: usize) -> usize {
    let mut utf16 = 0;
    for (unicode, c) in text.chars().enumerate() {
        if utf16 == position {
            return unicode;
        }
        utf16 += c.len_utf16();
        assert!(utf16 <= position, "position splits a surrogate pair");
    }
    assert_eq!(utf16, position);
    text.chars().count()
}

fn main() {
    let args: Vec<_> = env::args().collect();
    if args[1] == "stale-cursor" {
        let doc = LoroDoc::new();
        let text = doc.get_text("text");
        text.insert(0, "abc").unwrap();
        doc.commit();
        let cursor = text.get_cursor(1, Side::Middle).unwrap();
        text.delete(1, 1).unwrap();
        doc.commit();
        let shallow = LoroDoc::new();
        shallow
            .import(
                &doc.export(ExportMode::shallow_snapshot(&doc.state_frontiers()))
                    .unwrap(),
            )
            .unwrap();
        println!("{:?}", shallow.get_cursor_pos(&cursor));
        return;
    }
    if args[1] == "import" {
        let doc = LoroDoc::new();
        doc.set_peer_id(999).unwrap();
        doc.import(&fs::read(&args[2]).unwrap()).unwrap();
        let text = doc.get_text("text");
        let imported = text.to_string();
        text.insert_utf16(imported.encode_utf16().count(), "\n来自Rust🦀")
            .unwrap();
        doc.commit();
        fs::write(&args[3], doc.export(ExportMode::Snapshot).unwrap()).unwrap();
        println!(
            "{}",
            json!({"imported": imported, "edited": text.to_string()})
        );
        return;
    }
    let plan: Value = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let docs: Vec<_> = (1..=3)
        .map(|peer| {
            let doc = LoroDoc::new();
            doc.set_peer_id(peer).unwrap();
            doc
        })
        .collect();
    let mut packets = Vec::new();
    let mut undos = HashMap::new();
    let mut cursors: HashMap<String, Cursor> = HashMap::new();
    let mut trace = Vec::new();
    for step in plan.as_array().unwrap() {
        let peer = step["peer"].as_u64().unwrap() as usize;
        let doc = &docs[peer];
        let text = doc.get_text("text");
        let mut observed = Value::Null;
        match step["op"].as_str().unwrap() {
            "edit" => {
                let before = doc.oplog_vv();
                let at = step["at"].as_u64().unwrap() as usize;
                let delete = step["delete"].as_u64().unwrap() as usize;
                if delete > 0 {
                    text.delete_utf16(at, delete).unwrap();
                }
                text.insert_utf16(at, step["insert"].as_str().unwrap())
                    .unwrap();
                doc.commit();
                packets.push(doc.export(ExportMode::updates(&before)).unwrap());
            }
            "deliver" => {
                doc.import(&packets[step["packet"].as_u64().unwrap() as usize])
                    .unwrap();
            }
            "track" => {
                let mut undo = UndoManager::new(doc);
                undo.set_merge_interval(0);
                undos.insert(peer, undo);
            }
            "undo" | "redo" => {
                let undo = undos.get_mut(&peer).unwrap();
                observed = json!(if step["op"] == "undo" {
                    undo.undo().unwrap()
                } else {
                    undo.redo().unwrap()
                });
                packets.push(doc.export(ExportMode::all_updates()).unwrap());
            }
            "cursor" => {
                let at = utf16_to_unicode(&text.to_string(), step["at"].as_u64().unwrap() as usize);
                cursors.insert(
                    step["name"].as_str().unwrap().into(),
                    text.get_cursor(at, Side::Middle).unwrap(),
                );
            }
            "resolve" => {
                let cursor = &cursors[step["name"].as_str().unwrap()];
                let result = doc.get_cursor_pos(cursor).unwrap();
                let offset: usize = text
                    .to_string()
                    .chars()
                    .take(result.current.pos)
                    .map(char::len_utf16)
                    .sum();
                observed = json!({"offset": offset, "side": result.current.side as i8});
            }
            op => panic!("unknown operation: {op}"),
        }
        trace.push(json!({
            "texts": docs.iter().map(|d| d.get_text("text").to_string()).collect::<Vec<_>>(),
            "observed": observed,
        }));
    }
    fs::write(&args[3], docs[0].export(ExportMode::Snapshot).unwrap()).unwrap();
    println!("{}", json!({"trace": trace}));
}
