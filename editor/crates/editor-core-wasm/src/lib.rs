//! Thin bindings for the language-independent kernel, built independently of
//! the Notist language Wasm module. JSON keeps 64-bit peer IDs as strings.
use notist_editor_core::*;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::Receiver;
use wasm_bindgen::prelude::*;

fn encode(value: impl Serialize) -> String {
    serde_json::to_string(&value).unwrap()
}
fn decode<T: for<'a> Deserialize<'a>>(json: &str) -> Result<T, JsValue> {
    serde_json::from_str(json).map_err(|e| {
        JsValue::from_str(&encode(
            serde_json::json!({"code":"invalid_request", "message":e.to_string()}),
        ))
    })
}
fn error(e: CoreError) -> JsValue {
    JsValue::from_str(&encode(&e))
}
fn peer(value: Option<String>) -> Result<Option<u64>, JsValue> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| JsValue::from_str("invalid decimal writer id"))
        })
        .transpose()
}

#[wasm_bindgen]
pub struct EditorDocument {
    document: Document,
    events: Receiver<ChangeEvent>,
}

#[wasm_bindgen]
impl EditorDocument {
    #[wasm_bindgen(constructor)]
    pub fn new(
        identity: &str,
        writer: Option<String>,
        initial: &str,
    ) -> Result<EditorDocument, JsValue> {
        let mut document =
            Document::new(decode(identity)?, peer(writer)?, initial).map_err(error)?;
        let events = document.subscribe();
        Ok(Self { document, events })
    }

    pub fn from_snapshot(packet: &str, writer: Option<String>) -> Result<EditorDocument, JsValue> {
        let mut document =
            Document::from_snapshot(&decode(packet)?, peer(writer)?).map_err(error)?;
        let events = document.subscribe();
        Ok(Self { document, events })
    }
    pub fn snapshot(&self) -> String {
        encode(self.document.snapshot())
    }
    pub fn writer_id(&self) -> String {
        self.document.writer_id()
    }
    pub fn undo_state(&self) -> String {
        encode(self.document.undo_state())
    }
    pub fn transact(&mut self, transaction: &str) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .transact(decode(transaction)?)
                .map_err(error)?,
        ))
    }
    pub fn undo(&mut self, metadata: &str) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .undo_with_context(decode(metadata)?)
                .map_err(error)?,
        ))
    }
    pub fn redo(&mut self, metadata: &str) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .redo_with_context(decode(metadata)?)
                .map_err(error)?,
        ))
    }
    pub fn begin_undo_group(&mut self) -> Result<(), JsValue> {
        self.document.begin_undo_group().map_err(error)
    }
    pub fn end_undo_group(&mut self) {
        self.document.end_undo_group();
    }
    pub fn clear_undo(&mut self) {
        self.document.clear_undo();
    }
    pub fn export_snapshot(&self) -> Result<String, JsValue> {
        Ok(encode(self.document.export_snapshot().map_err(error)?))
    }
    pub fn export_updates_since(&self, version: &str) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .export_updates_since(&decode(version)?)
                .map_err(error)?,
        ))
    }
    pub fn import_updates(&mut self, packet: &str, origin: String) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .import(&decode(packet)?, origin)
                .map_err(error)?,
        ))
    }
    pub fn anchor_at(&self, offset: f64, affinity: &str) -> Result<String, JsValue> {
        // A direct JS -> usize binding silently truncates fractions and wraps
        // large offsets before the kernel can validate them.
        if !offset.is_finite()
            || offset.fract() != 0.0
            || offset < 0.0
            || offset > usize::MAX as f64
        {
            return Err(JsValue::from_str(&encode(serde_json::json!({
                "code": "invalid_request", "message": "anchor offset must be a non-negative integer within the platform index range"
            }))));
        }
        Ok(encode(
            self.document
                .anchor_at(offset as usize, decode(affinity)?)
                .map_err(error)?,
        ))
    }
    pub fn resolve_anchor(&self, anchor: &str) -> Result<String, JsValue> {
        Ok(encode(
            self.document
                .resolve_anchor(&decode(anchor)?)
                .map_err(error)?,
        ))
    }
    /// Drain only after a mutating call returns, so host callbacks cannot
    /// reenter the same borrowed Wasm object during a commit.
    pub fn take_events(&self) -> String {
        encode(self.events.try_iter().collect::<Vec<_>>())
    }
}
