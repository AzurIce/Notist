//! Browser editor bindings for the `notist-analysis` debug snapshot API.
//!
//! 薄封装：`analyze` 的语义（含失败时返回带 `failure` 诊断的空快照）完全在语言核
//! `notist_analysis::debug` 内，这里只做 JSON 字符串的进出。

use wasm_bindgen::prelude::*;

/// Analyze a SNAPSHOT.md request and return the snapshot JSON string.
#[wasm_bindgen]
pub fn analyze(request_json: &str) -> String {
    notist_analysis::debug::analyze(request_json).to_string()
}
