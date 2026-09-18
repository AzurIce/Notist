//! 实验性语言核：从 `.notc` 源文本到唯一权威产物的最小完整切片。
//!
//! 分层：语法层（`ast`）只管结构，意图层（`hir`）带绑定结论，物质层（`ir`）只含完成态。
//! 两段前端在语法层汇聚：`.notc` 直接解析，Markup 经 `desugar` 脱糖；此后走同一条
//! `resolve` → `reduce` 路径。
//!
//! 本切片要立住的三条不变量：
//! - 值域只含完成态：表达式（`hir::Expr`）与值（`ir::Value`）是两个类型，未规约态不进值域；
//! - 严格底向上：内容表达式先归约到终态，再分发本节点；
//! - 失败降级：绑定失败产出以该调用为名的可见 Item，内容不丢，只多一条 warn。

pub mod ast;
pub mod code;
pub mod desugar;
pub mod dump;
pub mod hir;
pub mod ir;
pub mod package;
pub mod reduce;
pub mod registry;
pub mod resolve;
pub mod syntax;
pub mod types;

pub use ir::{
    Content, CoreElement, Diagnostic, ElementName, Flow, Item, ItemState, ModuleResult, Severity,
    Value,
};

/// Markup 前端：源文本 → 语法层 → 意图层 → 物质层。
pub fn compile(source: &str) -> ModuleResult {
    let surface = syntax::parse(source);
    let body = desugar::desugar(&surface);
    let mut module = resolve::resolve(ast::Module { body });
    module.diagnostics.splice(0..0, surface.diagnostics);
    reduce::reduce(&module)
}

/// `.notc`：模块体是语句序列，解析出来就是意图层，没有脱糖这一步。
pub fn compile_code(source: &str) -> ModuleResult {
    let (body, diagnostics) = code::parse(source);
    let mut module = resolve::resolve(ast::Module { body });
    module.diagnostics.splice(0..0, diagnostics);
    reduce::reduce(&module)
}
