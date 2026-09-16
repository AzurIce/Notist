//! 实验性 IR：从源文本到唯一权威产物的最小切片。
//!
//! 这个 crate 是设计试验场，不参与既有管线，旧 crate 一律不动。
//! 切片范围：text、parbreak、强调族、section、未注册调用与绑定失败的降级，加一份可对拍的 dump。
//! 明确不做：list-item 的缩进规则、标注与属性、身份表与引用、跨模块、插件、投影视图、增量。
//!
//! 本切片要立住的三条不变量：
//! - 值域只含完成态：表达式（[`plan::Expr`]）与值（[`ir::Value`]）是两个类型，未规约态不进值域；
//! - 严格底向上：内容表达式先归约到终态，再分发本节点；
//! - 失败降级：绑定失败产出以该调用为名的可见 Item，内容不丢，只多一条 warn。

pub mod code;
pub mod dump;
pub mod ir;
pub mod package;
pub mod plan;
pub mod reduce;
pub mod registry;
pub mod syntax;

pub use ir::{
    Content, CoreElement, Diagnostic, ElementName, Flow, Item, ItemState, ModuleResult, Severity,
    Value,
};

/// 跑完整条管线：源文本 → 前端 → 意图层 → 物质层。
pub fn compile(source: &str) -> ModuleResult {
    let surface = syntax::parse(source);
    let planned = plan::plan(&surface);
    reduce::reduce(&planned)
}

/// `.notc`：模块体是语句序列，解析出来就是意图层，没有脱糖这一步。
pub fn compile_code(source: &str) -> ModuleResult {
    let (body, diagnostics) = code::parse(source);
    reduce::reduce(&plan::PlannedModule { body, diagnostics })
}
