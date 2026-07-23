#![allow(dead_code)]

use notist_model::{Content, Element, TableAlignment, TableLayoutError, table_layout};
use notist_syntax::StringLiteralForm;

use crate::{
    EvalDiagnostic, Function, FunctionContext, FunctionInput, FunctionOutput, FunctionRegistry,
    FunctionSignature, RegistryError,
};

pub(crate) fn register_builtins(registry: &mut FunctionRegistry) -> Result<(), RegistryError> {
    registry.register(TextFunction)?;
    registry.register(ParagraphFunction)?;
    registry.register(RefFunction)?;
    registry.register(HeadingFunction)?;
    registry.register(RawFunction)?;
    registry.register(CodeFunction)?;
    registry.register(QuoteFunction)?;
    registry.register(CalloutFunction)?;
    registry.register(DetailsFunction)?;
    registry.register(ListFunction)?;
    registry.register(EnumFunction)?;
    registry.register(ListItemFunction)?;
    registry.register(EnumItemFunction)?;
    registry.register(TaskFunction)?;
    registry.register(TaskItemFunction)?;
    registry.register(TableCellFunction)?;
    registry.register(TableFunction)?;
    registry.register(StrongFunction)?;
    registry.register(EmphFunction)?;
    registry.register(StrikeFunction)?;
    registry.register(UnderlineFunction)?;
    registry.register(KeyboardFunction)?;
    registry.register(MathFunction)?;
    registry.register(LinkFunction)?;
    registry.register(ImageFunction)?;
    registry.register(FigureFunction)?;
    registry.register(LinebreakFunction)?;
    registry.register(ParbreakFunction)?;
    registry.register(RuleFunction)?;
    registry.register(PagebreakFunction)?;
    Ok(())
}

struct TextFunction;

impl Function for TextFunction {
    fn name(&self) -> &str {
        "text"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::text_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Text(input.arguments.string("value").to_owned()),
            input.range,
        )))
    }
}

struct ParagraphFunction;

impl Function for ParagraphFunction {
    fn name(&self) -> &str {
        "paragraph"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let body = input.arguments.take_content("body");
        if body.is_empty() || body.elements.iter().any(|node| !node.element.is_inline()) {
            return Err(vec![EvalDiagnostic {
                message: "paragraph body must contain non-empty inline content".into(),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::Paragraph(body),
            input.range,
        )))
    }
}

struct RefFunction;

impl Function for RefFunction {
    fn name(&self) -> &str {
        "ref"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::ref_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let target = input.arguments.string("target");
        let reference = notist_syntax::parse_wiki_reference(target).map_err(|message| {
            vec![EvalDiagnostic {
                message,
                range: input.range,
            }]
        })?;
        Ok(FunctionOutput::content(Content::single(
            Element::Reference(reference),
            input.range,
        )))
    }
}

struct HeadingFunction;

impl Function for HeadingFunction {
    fn name(&self) -> &str {
        "heading"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::heading_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let level = input.arguments.int("level");
        if !(1..=6).contains(&level) {
            return Err(vec![EvalDiagnostic {
                message: "heading level must be between 1 and 6".into(),
                range: input.range,
            }]);
        }
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Heading {
                level: level as u8,
                body,
            },
            input.range,
        )))
    }
}

struct OutlineFunction;

impl Function for OutlineFunction {
    fn name(&self) -> &str {
        "outline"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::outline_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let depth = input.arguments.int("depth");
        if !(1..=6).contains(&depth) {
            return Err(vec![EvalDiagnostic {
                message: "outline depth must be between 1 and 6".into(),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::Outline { depth: depth as u8 },
            input.range,
        )))
    }
}

struct RawFunction;

impl Function for RawFunction {
    fn name(&self) -> &str {
        "raw"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::raw_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let language = input.arguments.optional_string("lang").map(str::to_owned);
        let text = input.arguments.string("text").to_owned();
        let block = input.arguments.string_form("text") == Some(StringLiteralForm::Multiline);
        Ok(FunctionOutput::content(raw_content(
            text,
            block,
            language,
            input.range,
        )))
    }
}

struct CodeFunction;

impl Function for CodeFunction {
    fn name(&self) -> &str {
        "code"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::code_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(raw_content(
            input.arguments.string("text").to_owned(),
            input.arguments.bool("block"),
            input.arguments.optional_string("lang").map(str::to_owned),
            input.range,
        )))
    }
}

struct QuoteFunction;

impl Function for QuoteFunction {
    fn name(&self) -> &str {
        "quote"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::quote_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let attribution = input.arguments.take_optional_content("attribution");
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Quote { body, attribution },
            input.range,
        )))
    }
}

struct CalloutFunction;

impl Function for CalloutFunction {
    fn name(&self) -> &str {
        "callout"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::callout_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let kind = input.arguments.string("kind").trim().to_owned();
        if kind.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "callout kind cannot be empty".into(),
                range: input.range,
            }]);
        }
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Callout {
                kind,
                title: input.arguments.take_optional_content("title"),
                body,
            },
            input.range,
        )))
    }
}

struct DetailsFunction;

impl Function for DetailsFunction {
    fn name(&self) -> &str {
        "details"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::details_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let summary = input.arguments.take_optional_content("summary");
        let open = input.arguments.bool("open");
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Details {
                summary,
                open,
                body,
            },
            input.range,
        )))
    }
}

struct ListFunction;

impl Function for ListFunction {
    fn name(&self) -> &str {
        "list"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::list_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        list_container(input, false)
    }
}

struct EnumFunction;

impl Function for EnumFunction {
    fn name(&self) -> &str {
        "enum"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::list_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        list_container(input, true)
    }
}

fn list_container(
    mut input: FunctionInput<'_>,
    ordered: bool,
) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
    let mut body = input.arguments.take_content("body");
    body.elements
        .retain(|node| !matches!(&node.element, Element::Text(text) if text.trim().is_empty()));
    let valid = if ordered {
        body.elements
            .iter()
            .all(|node| matches!(node.element, Element::EnumItem { .. }))
    } else {
        body.elements
            .iter()
            .all(|node| matches!(node.element, Element::ListItem(_)))
    };
    if body.elements.is_empty() || !valid {
        let container = if ordered { "enum" } else { "list" };
        let item = if ordered { "enum::item" } else { "list::item" };
        return Err(vec![EvalDiagnostic {
            message: format!("{container} body must contain at least one {item}"),
            range: input.range,
        }]);
    }
    Ok(FunctionOutput::content(Content::single(
        Element::List {
            ordered,
            items: body.elements,
        },
        input.range,
    )))
}

struct ListItemFunction;

impl Function for ListItemFunction {
    fn name(&self) -> &str {
        "list::item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::list_item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::ListItem(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

struct EnumItemFunction;

impl Function for EnumItemFunction {
    fn name(&self) -> &str {
        "enum::item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::enum_item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let value = input.arguments.optional_int("value");
        if value.is_some_and(|value| !(1..=u32::MAX as i64).contains(&value)) {
            return Err(vec![EvalDiagnostic {
                message: format!("enum item value must be between 1 and {}", u32::MAX),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::EnumItem {
                value: value.map(|value| value as u32),
                body: input.arguments.take_content("body"),
            },
            input.range,
        )))
    }
}

struct TermsFunction;

impl Function for TermsFunction {
    fn name(&self) -> &str {
        "terms"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::list_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        item_container(
            input,
            "terms",
            "terms::item",
            |element| matches!(element, Element::TermItem { .. }),
            |items| Element::Terms { items },
        )
    }
}

struct TaskFunction;

impl Function for TaskFunction {
    fn name(&self) -> &str {
        "task"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::list_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        item_container(
            input,
            "task",
            "task::item",
            |element| matches!(element, Element::TaskItem { .. }),
            |items| Element::Tasks { items },
        )
    }
}

fn item_container(
    mut input: FunctionInput<'_>,
    container: &str,
    item: &str,
    accepts: fn(&Element) -> bool,
    construct: fn(Vec<notist_model::ElementNode>) -> Element,
) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
    let mut body = input.arguments.take_content("body");
    body.elements
        .retain(|node| !matches!(&node.element, Element::Text(text) if text.trim().is_empty()));
    if body.elements.is_empty() || !body.elements.iter().all(|node| accepts(&node.element)) {
        return Err(vec![EvalDiagnostic {
            message: format!("{container} body must contain at least one {item}"),
            range: input.range,
        }]);
    }
    Ok(FunctionOutput::content(Content::single(
        construct(body.elements),
        input.range,
    )))
}

struct TermItemFunction;

impl Function for TermItemFunction {
    fn name(&self) -> &str {
        "terms::item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::terms_item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let term = input.arguments.take_content("term");
        let description = input.arguments.take_content("description");
        Ok(FunctionOutput::content(Content::single(
            Element::TermItem { term, description },
            input.range,
        )))
    }
}

struct TaskItemFunction;

impl Function for TaskItemFunction {
    fn name(&self) -> &str {
        "task::item"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::task_item_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let checked = input.arguments.bool("checked");
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::TaskItem { checked, body },
            input.range,
        )))
    }
}

struct TableCellFunction;

impl Function for TableCellFunction {
    fn name(&self) -> &str {
        "table::cell"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::table_cell_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let colspan = input.arguments.int("colspan");
        let rowspan = input.arguments.int("rowspan");
        if !(1..=u16::MAX as i64).contains(&colspan) {
            return Err(vec![EvalDiagnostic {
                message: "table cell colspan must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        if !(1..=u16::MAX as i64).contains(&rowspan) {
            return Err(vec![EvalDiagnostic {
                message: "table cell rowspan must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::TableCell {
                body: input.arguments.take_content("body"),
                colspan: colspan as u16,
                rowspan: rowspan as u16,
            },
            input.range,
        )))
    }
}

struct TableFunction;

impl Function for TableFunction {
    fn name(&self) -> &str {
        "table"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::table_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let columns = input.arguments.int("columns");
        let header = input.arguments.bool("header");
        if columns <= 0 || columns > u16::MAX as i64 {
            return Err(vec![EvalDiagnostic {
                message: "table columns must be between 1 and 65535".into(),
                range: input.range,
            }]);
        }
        let alignments =
            match table_alignments(input.arguments.optional_string("align"), columns as usize) {
                Ok(alignments) => alignments,
                Err(message) => {
                    return Err(vec![EvalDiagnostic {
                        message,
                        range: input.range,
                    }]);
                }
            };
        let caption = input.arguments.take_optional_content("caption");
        let body = input.arguments.take_content("body");
        if body.elements.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "table requires at least one table::cell".into(),
                range: input.range,
            }]);
        }
        if body
            .elements
            .iter()
            .any(|node| !matches!(node.element, Element::TableCell { .. }))
        {
            return Err(vec![EvalDiagnostic {
                message: "table body may contain only table::cell elements".into(),
                range: input.range,
            }]);
        }
        if let Err(error) = table_layout(columns as u16, &body.elements) {
            return Err(vec![EvalDiagnostic {
                message: table_layout_message(error, columns as u16),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::Table {
                columns: columns as u16,
                header,
                alignments,
                caption,
                cells: body.elements,
            },
            input.range,
        )))
    }
}

fn table_layout_message(error: TableLayoutError, columns: u16) -> String {
    match error {
        TableLayoutError::NonCell { cell } => {
            format!("table cell {cell} is not a table::cell element")
        }
        TableLayoutError::CellDoesNotFit {
            row,
            cell,
            column,
            colspan,
        } => format!(
            "table cell {cell} with colspan {colspan} does not fit row {row} at column {} of {columns}",
            column + 1
        ),
        TableLayoutError::IncompleteRow { row } => {
            format!("table row {row} does not fill all {columns} columns")
        }
        TableLayoutError::FullyCoveredRow { row } => {
            format!("table row {row} is fully covered by rowspans and cannot contain the next cell")
        }
        TableLayoutError::RowspanBeyondTable => {
            "table rowspan extends beyond the final explicit row".into()
        }
    }
}

fn table_alignments(source: Option<&str>, columns: usize) -> Result<Vec<TableAlignment>, String> {
    let Some(source) = source else {
        return Ok(vec![TableAlignment::Default; columns]);
    };
    let alignments: Result<Vec<_>, _> = source
        .split(',')
        .map(|value| match value.trim() {
            "default" | "" => Ok(TableAlignment::Default),
            "left" => Ok(TableAlignment::Left),
            "center" => Ok(TableAlignment::Center),
            "right" => Ok(TableAlignment::Right),
            value => Err(format!("unknown table alignment `{value}`")),
        })
        .collect();
    let alignments = alignments?;
    if alignments.len() != columns {
        return Err(format!(
            "table align specifies {} columns, expected {columns}",
            alignments.len()
        ));
    }
    Ok(alignments)
}

struct StrongFunction;

impl Function for StrongFunction {
    fn name(&self) -> &str {
        "strong"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Strong(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

struct EmphFunction;

impl Function for EmphFunction {
    fn name(&self) -> &str {
        "emph"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Emph(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

struct StrikeFunction;

impl Function for StrikeFunction {
    fn name(&self) -> &str {
        "strike"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::inline_body_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Strike(input.arguments.take_content("body")),
            input.range,
        )))
    }
}

macro_rules! inline_wrapper_function {
    ($function:ident, $name:literal, $variant:ident) => {
        struct $function;

        impl Function for $function {
            fn name(&self) -> &str {
                $name
            }

            fn signature(&self) -> FunctionSignature {
                notist_model::inline_body_signature()
            }

            fn call(
                &self,
                _context: &FunctionContext<'_>,
                mut input: FunctionInput<'_>,
            ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
                Ok(FunctionOutput::content(Content::single(
                    Element::$variant(input.arguments.take_content("body")),
                    input.range,
                )))
            }
        }
    };
}

inline_wrapper_function!(HighlightFunction, "highlight", Highlight);
inline_wrapper_function!(UnderlineFunction, "underline", Underline);
inline_wrapper_function!(KeyboardFunction, "kbd", Keyboard);
inline_wrapper_function!(SampleFunction, "samp", Sample);
inline_wrapper_function!(InsertFunction, "insert", Insert);
inline_wrapper_function!(SpoilerFunction, "spoiler", Spoiler);
inline_wrapper_function!(SuperFunction, "super", Super);
inline_wrapper_function!(SubFunction, "sub", Sub);
inline_wrapper_function!(FootnoteFunction, "footnote", Footnote);
inline_wrapper_function!(CommentFunction, "comment", Comment);

struct MathFunction;

struct AbbrFunction;

impl Function for AbbrFunction {
    fn name(&self) -> &str {
        "abbr"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::abbr_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Abbr {
                term: input.arguments.string("term").to_owned(),
                expansion: input.arguments.string("expansion").to_owned(),
            },
            input.range,
        )))
    }
}

struct CiteFunction;

struct TimeFunction;

impl Function for TimeFunction {
    fn name(&self) -> &str {
        "time"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::time_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let datetime = input.arguments.string("datetime").trim();
        if datetime.is_empty() {
            return Err(vec![EvalDiagnostic {
                message: "time datetime must not be empty".into(),
                range: input.range,
            }]);
        }
        Ok(FunctionOutput::content(Content::single(
            Element::Time {
                datetime: datetime.to_owned(),
                body: input.arguments.take_content("body"),
            },
            input.range,
        )))
    }
}

impl Function for CiteFunction {
    fn name(&self) -> &str {
        "cite"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::cite_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let key = input.arguments.string("key").trim();
        if !citation_key_is_valid(key) {
            return Err(vec![EvalDiagnostic {
                message:
                    "citation key must be non-empty and contain no whitespace, comma, or brackets"
                        .into(),
                range: input.range,
            }]);
        }
        let locator = input
            .arguments
            .optional_string("locator")
            .map(str::trim)
            .filter(|locator| !locator.is_empty())
            .map(str::to_owned);
        Ok(FunctionOutput::content(Content::single(
            Element::Citation {
                key: key.to_owned(),
                locator,
            },
            input.range,
        )))
    }
}

pub(crate) fn citation_key_is_valid(key: &str) -> bool {
    !key.is_empty()
        && key.chars().all(|character| {
            !character.is_whitespace()
                && !character.is_control()
                && !matches!(character, '[' | ']' | ',')
        })
}

impl Function for MathFunction {
    fn name(&self) -> &str {
        "math"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::math_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Math {
                text: input.arguments.string("text").to_owned(),
                block: input.arguments.bool("block"),
            },
            input.range,
        )))
    }
}

struct LinkFunction;

impl Function for LinkFunction {
    fn name(&self) -> &str {
        "link"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::link_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let destination = input.arguments.string("destination").to_owned();
        let title = input.arguments.optional_string("title").map(str::to_owned);
        let body = input.arguments.take_content("body");
        Ok(FunctionOutput::content(Content::single(
            Element::Link {
                destination,
                title,
                body,
            },
            input.range,
        )))
    }
}

struct ImageFunction;

impl Function for ImageFunction {
    fn name(&self) -> &str {
        "image"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::image_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let source = input.arguments.string("source").to_owned();
        let alt = input.arguments.string("alt").to_owned();
        let title = input.arguments.optional_string("title").map(str::to_owned);
        let width = image_dimension(input.arguments.optional_int("width"), "width", input.range)?;
        let height = image_dimension(
            input.arguments.optional_int("height"),
            "height",
            input.range,
        )?;
        Ok(FunctionOutput::content(Content::single(
            Element::Image {
                source,
                alt,
                title,
                width,
                height,
            },
            input.range,
        )))
    }
}

struct FigureFunction;

impl Function for FigureFunction {
    fn name(&self) -> &str {
        "figure"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::figure_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        mut input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let source = input.arguments.string("source").to_owned();
        let alt = input.arguments.string("alt").to_owned();
        let title = input.arguments.optional_string("title").map(str::to_owned);
        let caption = input.arguments.take_content("caption");
        Ok(FunctionOutput::content(Content::single(
            Element::Figure {
                source,
                alt,
                title,
                caption,
            },
            input.range,
        )))
    }
}

struct VideoFunction;

impl Function for VideoFunction {
    fn name(&self) -> &str {
        "video"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::video_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        let source = input.arguments.string("source").to_owned();
        let poster = input.arguments.optional_string("poster").map(str::to_owned);
        let controls = input.arguments.bool("controls");
        Ok(FunctionOutput::content(Content::single(
            Element::Video {
                source,
                poster,
                controls,
            },
            input.range,
        )))
    }
}

struct AudioFunction;

impl Function for AudioFunction {
    fn name(&self) -> &str {
        "audio"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::audio_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Audio {
                source: input.arguments.string("source").to_owned(),
                controls: input.arguments.bool("controls"),
                looping: input.arguments.bool("loop"),
            },
            input.range,
        )))
    }
}

fn image_dimension(
    value: Option<i64>,
    name: &str,
    range: notist_model::TextRange,
) -> Result<Option<u32>, Vec<EvalDiagnostic>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !(1..=u32::MAX as i64).contains(&value) {
        return Err(vec![EvalDiagnostic {
            message: format!("image {name} must be between 1 and {}", u32::MAX),
            range,
        }]);
    }
    Ok(Some(value as u32))
}

struct LinebreakFunction;

impl Function for LinebreakFunction {
    fn name(&self) -> &str {
        "linebreak"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Linebreak,
            input.range,
        )))
    }
}

struct ParbreakFunction;

impl Function for ParbreakFunction {
    fn name(&self) -> &str {
        "parbreak"
    }
    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }
    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Parbreak,
            input.range,
        )))
    }
}

struct RuleFunction;

impl Function for RuleFunction {
    fn name(&self) -> &str {
        "rule"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Rule,
            input.range,
        )))
    }
}

struct PagebreakFunction;

impl Function for PagebreakFunction {
    fn name(&self) -> &str {
        "pagebreak"
    }

    fn signature(&self) -> FunctionSignature {
        notist_model::empty_content_signature()
    }

    fn call(
        &self,
        _context: &FunctionContext<'_>,
        input: FunctionInput<'_>,
    ) -> Result<FunctionOutput, Vec<EvalDiagnostic>> {
        Ok(FunctionOutput::content(Content::single(
            Element::Pagebreak,
            input.range,
        )))
    }
}

pub(crate) fn raw_content(
    text: String,
    block: bool,
    language: Option<String>,
    range: notist_model::TextRange,
) -> Content {
    Content::single(
        Element::Raw {
            text,
            block,
            language,
        },
        range,
    )
}

#[cfg(test)]
mod tests {
    use notist_model::{Element, ElementNode, TableAlignment};

    use crate::Evaluator;

    #[test]
    fn excludes_plugin_candidates_from_core_registry() {
        let evaluator = Evaluator::default();
        for name in [
            "outline",
            "terms",
            "insert",
            "spoiler",
            "highlight",
            "samp",
            "super",
            "sub",
            "footnote",
            "comment",
            "abbr",
            "time",
            "cite",
            "video",
            "audio",
        ] {
            let evaluation = evaluator.evaluate(&format!("#{name}[]"));
            assert!(
                evaluation
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message == format!("unknown function `{name}`")),
                "{name}: {:?}",
                evaluation.diagnostics
            );
        }
    }

    #[test]
    fn evaluates_text_function_without_reparsing_markup() {
        let evaluated = Evaluator::default().evaluate("#text(\"*literal*\")");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Text(text) if text == "*literal*"
        ));
    }

    #[test]
    fn evaluates_paragraph_function_and_rejects_block_content() {
        let evaluated = Evaluator::default().evaluate("#paragraph[plain *content*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Paragraph(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let block = Evaluator::default().evaluate("#paragraph[First\n\nSecond]");
        assert!(
            block
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("non-empty inline content") })
        );
        let empty = Evaluator::default().evaluate("#paragraph[]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("non-empty inline content") })
        );
    }

    #[test]
    fn evaluates_ref_function_with_wiki_reference_validation() {
        let evaluator = Evaluator::default();
        let explicit = evaluator.evaluate("#ref(\"vault::guide::intro#overview\")");
        let sugar = evaluator.evaluate("[[vault::guide::intro#overview]]");
        assert!(
            explicit.diagnostics.is_empty(),
            "{:?}",
            explicit.diagnostics
        );
        assert!(sugar.diagnostics.is_empty(), "{:?}", sugar.diagnostics);
        assert_eq!(
            explicit.content.elements[0].element,
            sugar.content.elements[0].element
        );

        let invalid = evaluator.evaluate("#ref(\"vault::::guide\")");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("empty segment") })
        );
    }

    #[test]
    fn evaluates_heading_raw_code_and_quote() {
        let evaluator = Evaluator::default();

        let heading = evaluator.evaluate("#heading(level=2)[Title]");
        assert!(heading.diagnostics.is_empty(), "{:?}", heading.diagnostics);
        assert!(matches!(
            &heading.content.elements[0].element,
            Element::Heading { level: 2, .. }
        ));

        let raw = evaluator.evaluate("#raw(r#\"fn main() {}\"#, lang=\"rust\")");
        assert!(raw.diagnostics.is_empty(), "{:?}", raw.diagnostics);
        assert!(matches!(
            &raw.content.elements[0].element,
            Element::Raw { text, language, block: false }
                if text == "fn main() {}" && language.as_deref() == Some("rust")
        ));

        let code = evaluator.evaluate("#code(\"fn main() {}\", lang=\"rust\", block=true)");
        assert!(code.diagnostics.is_empty(), "{:?}", code.diagnostics);
        assert!(matches!(
            &code.content.elements[0].element,
            Element::Raw { text, language, block: true }
                if text == "fn main() {}" && language.as_deref() == Some("rust")
        ));

        let quote = evaluator.evaluate("#quote[Quoted [[self::target]]]");
        assert!(quote.diagnostics.is_empty(), "{:?}", quote.diagnostics);
        assert!(matches!(
            &quote.content.elements[0].element,
            Element::Quote { body, .. } if matches!(body.elements[1].element, Element::Reference(_))
        ));
    }

    #[test]
    fn evaluates_ordered_and_unordered_list_items() {
        let evaluator = Evaluator::default();
        let unordered = evaluator.evaluate("#list::item[One]");
        assert!(
            unordered.diagnostics.is_empty(),
            "{:?}",
            unordered.diagnostics
        );
        assert!(matches!(
            unordered.content.elements[0].element,
            Element::ListItem(_)
        ));

        let ordered = evaluator.evaluate("#enum::item[First]");
        assert!(ordered.diagnostics.is_empty(), "{:?}", ordered.diagnostics);
        assert!(matches!(
            ordered.content.elements[0].element,
            Element::EnumItem { .. }
        ));

        let valued = evaluator.evaluate("#enum::item(value=4)[Fourth]");
        assert!(valued.diagnostics.is_empty(), "{:?}", valued.diagnostics);
        assert!(matches!(
            valued.content.elements[0].element,
            Element::EnumItem { value: Some(4), .. }
        ));

        let list = evaluator.evaluate(
            "#list[#list::item[One]#list::item[Two]]#enum[#enum::item(value=3)[Three]#enum::item[Four]]",
        );
        assert!(list.diagnostics.is_empty(), "{:?}", list.diagnostics);
        assert!(matches!(
            &list.content.elements[0].element,
            Element::List { ordered: false, items } if items.len() == 2
        ));
        assert!(matches!(
            &list.content.elements[1].element,
            Element::List { ordered: true, items } if items.len() == 2
        ));

        let wrong_item = evaluator.evaluate("#list[#enum::item[Wrong]]");
        assert!(
            wrong_item
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("at least one list::item") })
        );
        let empty = evaluator.evaluate("#enum[]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("at least one enum::item") })
        );
    }

    #[test]
    fn evaluates_callouts() {
        let evaluator = Evaluator::default();
        let default = evaluator.evaluate("#callout[Remember this]");
        assert!(default.diagnostics.is_empty(), "{:?}", default.diagnostics);
        assert!(matches!(
            &default.content.elements[0].element,
            Element::Callout { kind, title: None, body }
                if kind == "note" && matches!(&body.elements[0].element, Element::Text(text) if text == "Remember this")
        ));

        let titled = evaluator.evaluate("#callout(kind=\"warning\", title=[Risk])[Body]");
        assert!(titled.diagnostics.is_empty(), "{:?}", titled.diagnostics);
        assert!(matches!(
            &titled.content.elements[0].element,
            Element::Callout { title: Some(title), .. }
                if matches!(&title.elements[0].element, Element::Text(text) if text == "Risk")
        ));

        let empty = evaluator.evaluate("#callout(kind=\"\")[Body]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("kind cannot be empty"))
        );
    }

    #[test]
    fn evaluates_details() {
        let evaluated =
            Evaluator::default().evaluate("#details(summary=[More], open=true)[Hidden *content*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Details { summary: Some(summary), open: true, body }
                if matches!(&summary.elements[0].element, Element::Text(text) if text == "More")
                    && body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));

        let without_summary = Evaluator::default().evaluate("#details[Hidden]");
        assert!(
            without_summary.diagnostics.is_empty(),
            "{:?}",
            without_summary.diagnostics
        );
        assert!(matches!(
            &without_summary.content.elements[0].element,
            Element::Details {
                summary: None,
                open: false,
                ..
            }
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_definition_list_items() {
        let evaluated =
            Evaluator::default().evaluate("#terms::item(term=[API])[Application interface]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::TermItem { term, description }
                if matches!(&term.elements[0].element, Element::Text(text) if text == "API")
                && matches!(&description.elements[0].element, Element::Text(text) if text == "Application interface")
        ));

        let terms = Evaluator::default().evaluate(
            "#terms[#terms::item(term=[API])[Application interface]#terms::item(term=[URL])[Address]]",
        );
        assert!(terms.diagnostics.is_empty(), "{:?}", terms.diagnostics);
        assert!(matches!(
            &terms.content.elements[0].element,
            Element::Terms { items } if items.len() == 2
        ));
        let invalid = Evaluator::default().evaluate("#terms[#task::item[Wrong]]");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("at least one terms::item") })
        );
    }

    #[test]
    fn evaluates_task_items() {
        let evaluator = Evaluator::default();
        let incomplete = evaluator.evaluate("#task::item[Write tests]");
        assert!(
            incomplete.diagnostics.is_empty(),
            "{:?}",
            incomplete.diagnostics
        );
        assert!(matches!(
            incomplete.content.elements[0].element,
            Element::TaskItem { checked: false, .. }
        ));

        let complete = evaluator.evaluate("#task::item(checked=true)[Ship]");
        assert!(
            complete.diagnostics.is_empty(),
            "{:?}",
            complete.diagnostics
        );
        assert!(matches!(
            complete.content.elements[0].element,
            Element::TaskItem { checked: true, .. }
        ));

        let tasks =
            evaluator.evaluate("#task[#task::item[Write tests]#task::item(checked=true)[Ship]]");
        assert!(tasks.diagnostics.is_empty(), "{:?}", tasks.diagnostics);
        assert!(matches!(
            &tasks.content.elements[0].element,
            Element::Tasks { items } if items.len() == 2
        ));
        let invalid = evaluator.evaluate("#task[#list::item[Wrong]]");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("at least one task::item") })
        );
    }

    #[test]
    fn evaluates_tables_from_table_cells() {
        let evaluated = Evaluator::default().evaluate(
            "#table(columns=2)[#table::cell[One]#table::cell[Two]#table::cell[Three]#table::cell[Four]]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Table { columns: 2, header: false, cells, .. } if cells.len() == 4
        ));
    }

    #[test]
    fn evaluates_tables_with_explicit_headers() {
        let evaluated = Evaluator::default().evaluate(
            "#table(columns=2, header=true, align=\"left,right\")[#table::cell[Name]#table::cell[Value]#table::cell[one]#table::cell[two]]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Table { columns: 2, header: true, alignments, cells, .. }
                if cells.len() == 4
                    && alignments == &[TableAlignment::Left, TableAlignment::Right]
        ));
    }

    #[test]
    fn evaluates_table_caption() {
        let evaluated = Evaluator::default().evaluate(
            "#table(columns=2, caption=[*Inventory*])[#table::cell[Name]#table::cell[Value]]",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Table { caption: Some(caption), .. }
                if matches!(&caption.elements[0].element, Element::Strong(_))
        ));
    }

    #[test]
    fn rejects_malformed_tables() {
        let evaluator = Evaluator::default();
        let empty = evaluator.evaluate("#table(columns=2)[]");
        assert!(
            empty
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("at least one"))
        );
        let uneven = evaluator.evaluate("#table(columns=2)[#table::cell[One]]");
        assert!(
            uneven
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("does not fill all 2 columns"))
        );
        let spanning = evaluator.evaluate(
            "#table(columns=3)[#table::cell(colspan=2)[One]#table::cell[Two]#table::cell(rowspan=2)[Three]#table::cell(colspan=2)[Four]#table::cell[Five]#table::cell[Six]]",
        );
        assert!(
            spanning.diagnostics.is_empty(),
            "{:?}",
            spanning.diagnostics
        );
        assert!(matches!(
            &spanning.content.elements[0].element,
            Element::Table { cells, .. }
                if matches!(cells[0].element, Element::TableCell { colspan: 2, rowspan: 1, .. })
                    && matches!(cells[2].element, Element::TableCell { colspan: 1, rowspan: 2, .. })
        ));
        let bad_span = evaluator.evaluate("#table(columns=2)[#table::cell(colspan=3)[One]]");
        assert!(
            bad_span
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("does not fit row 1"))
        );
        let overlapping = evaluator.evaluate(
            "#table(columns=3)[#table::cell(colspan=2)[Wide]#table::cell(rowspan=2)[Tall]#table::cell[Next]#table::cell(colspan=2)[Overlap]]",
        );
        assert!(
            overlapping
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("does not fit row 2") })
        );
        let dangling_rowspan = evaluator
            .evaluate("#table(columns=2)[#table::cell(rowspan=2)[Tall]#table::cell[Only]]");
        assert!(dangling_rowspan.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("extends beyond the final explicit row")
        }));
        let bad_value =
            evaluator.evaluate("#table(columns=2)[#table::cell(colspan=0)[One]#table::cell[Two]]");
        assert!(
            bad_value
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("colspan must be between"))
        );
        let unknown_alignment =
            evaluator.evaluate("#table(columns=1, align=\"diagonal\")[#table::cell[One]]");
        assert!(
            unknown_alignment
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unknown table alignment"))
        );
        let wrong_alignment_count = evaluator
            .evaluate("#table(columns=2, align=\"left\")[#table::cell[One]#table::cell[Two]]");
        assert!(
            wrong_alignment_count
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expected 2"))
        );
    }

    #[test]
    fn evaluates_inline_element_functions() {
        let evaluator = Evaluator::default();
        let evaluated = evaluator.evaluate(
            "#strong[bold]#emph[slanted]#link(\"https://example.test\")[site]#linebreak()#parbreak()",
        );
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Strong(_)
        ));
        assert!(matches!(
            evaluated.content.elements[1].element,
            Element::Emph(_)
        ));
        assert!(matches!(
            evaluated.content.elements[2].element,
            Element::Link { ref destination, .. } if destination == "https://example.test"
        ));
        assert!(matches!(
            evaluated.content.elements[3].element,
            Element::Linebreak
        ));
        assert!(matches!(
            evaluated.content.elements[4].element,
            Element::Parbreak
        ));

        let titled = evaluator.evaluate("#link(\"https://example.test\", \"Example\")[site]");
        assert!(titled.diagnostics.is_empty(), "{:?}", titled.diagnostics);
        assert!(matches!(
            &titled.content.elements[0].element,
            Element::Link { title, .. } if title.as_deref() == Some("Example")
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_strike_insert_and_spoiler_functions() {
        let evaluated =
            Evaluator::default().evaluate("#strike[obsolete]#insert[replacement]#spoiler[ending]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Strike(body)
                if matches!(&body.elements[0].element, Element::Text(text) if text == "obsolete")
        ));
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Insert(body)
                if matches!(&body.elements[0].element, Element::Text(text) if text == "replacement")
        ));
        assert!(matches!(
            &evaluated.content.elements[2].element,
            Element::Spoiler(body)
                if matches!(&body.elements[0].element, Element::Text(text) if text == "ending")
        ));
    }

    #[test]
    fn evaluates_images() {
        let evaluator = Evaluator::default();
        let default_alt = evaluator.evaluate("#image(\"diagram.png\")");
        assert!(
            default_alt.diagnostics.is_empty(),
            "{:?}",
            default_alt.diagnostics
        );
        assert!(matches!(
            &default_alt.content.elements[0].element,
            Element::Image { source, alt, .. } if source == "diagram.png" && alt.is_empty()
        ));

        let described = evaluator.evaluate("#image(source=\"diagram.png\", alt=\"Flow\")");
        assert!(
            described.diagnostics.is_empty(),
            "{:?}",
            described.diagnostics
        );
        assert!(matches!(
            &described.content.elements[0].element,
            Element::Image { source, alt, .. } if source == "diagram.png" && alt == "Flow"
        ));
    }

    #[test]
    fn evaluates_image_metadata_and_rejects_invalid_dimensions() {
        let evaluator = Evaluator::default();
        let sized = evaluator.evaluate(
            "#image(source=\"diagram.png\", title=\"Flow chart\", width=640, height=480)",
        );
        assert!(sized.diagnostics.is_empty(), "{:?}", sized.diagnostics);
        assert!(matches!(
            &sized.content.elements[0].element,
            Element::Image { title, width: Some(640), height: Some(480), .. }
                if title.as_deref() == Some("Flow chart")
        ));

        let invalid = evaluator.evaluate("#image(\"diagram.png\", width=0)");
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("image width must be between 1"))
        );
    }

    #[test]
    fn evaluates_figures() {
        let evaluated = Evaluator::default()
            .evaluate("#figure(source=\"diagram.png\", alt=\"Diagram\", title=\"Flow\")[Caption]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Figure { source, alt, title, caption }
                if source == "diagram.png"
                    && alt == "Diagram"
                    && title.as_deref() == Some("Flow")
                    && matches!(&caption.elements[0].element, Element::Text(text) if text == "Caption")
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_videos() {
        let evaluated = Evaluator::default()
            .evaluate("#video(source=\"movie.mp4\", poster=\"poster.png\", controls=false)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Video { source, poster, controls: false }
                if source == "movie.mp4" && poster.as_deref() == Some("poster.png")
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_audio() {
        let evaluated = Evaluator::default()
            .evaluate("#audio(source=\"sound.ogg\", controls=false, loop=true)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Audio { source, controls: false, looping: true } if source == "sound.ogg"
        ));
    }

    #[test]
    fn evaluates_document_separator_functions() {
        let evaluated = Evaluator::default().evaluate("#rule()#pagebreak()");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements[0].element,
            Element::Rule
        ));
        assert!(matches!(
            evaluated.content.elements[1].element,
            Element::Pagebreak
        ));
    }

    #[test]
    fn block_raw_excludes_delimiter_line_breaks() {
        let evaluated = Evaluator::default()
            .evaluate("#raw(r#\"\"\"\nline one\nline two\n\"\"\"#, lang=\"text\")");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Raw { text, block: true, .. } if text == "line one\nline two"
        ));
    }

    #[test]
    fn raw_triple_quotes_without_an_opening_line_break_stay_inline() {
        let evaluated = Evaluator::default().evaluate(r####"#raw(r#"""quoted"""#)"####);
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Raw { text, block: false, .. } if text == "\"\"quoted\"\""
        ));
    }

    #[test]
    fn reports_signature_and_trailing_content_errors_before_calling_builtins() {
        let evaluator = Evaluator::default();
        let wrong_level = evaluator.evaluate("#heading(level=\"two\")[Title]");
        assert!(
            wrong_level.diagnostics[0]
                .message
                .contains("expected Int, found String")
        );

        let wrong_body = evaluator.evaluate("#raw[parsed]");
        assert_eq!(
            wrong_body.diagnostics[0].message,
            "function does not accept trailing Content"
        );

        let unknown = evaluator.evaluate("#quote(source=\"book\")[text]");
        assert_eq!(unknown.diagnostics[0].message, "unknown argument `source`");
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_additional_inline_wrapper_functions() {
        let evaluated =
            Evaluator::default().evaluate("#highlight[marked]#underline[under]#super[2]#sub[i]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            evaluated.content.elements.as_slice(),
            [
                ElementNode {
                    element: Element::Highlight(_),
                    ..
                },
                ElementNode {
                    element: Element::Underline(_),
                    ..
                },
                ElementNode {
                    element: Element::Super(_),
                    ..
                },
                ElementNode {
                    element: Element::Sub(_),
                    ..
                },
            ]
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_footnote_function() {
        let evaluated = Evaluator::default().evaluate("Text#footnote[Source *details*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Footnote(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_comment_function() {
        let evaluated = Evaluator::default().evaluate("Visible#comment[Author *note*]");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[1].element,
            Element::Comment(body)
                if body.elements.iter().any(|node| matches!(node.element, Element::Strong(_)))
        ));
    }

    #[test]
    fn evaluates_math_function() {
        let evaluated = Evaluator::default().evaluate("#math(text=\"x^2\", block=true)");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(
            matches!(&evaluated.content.elements[0].element, Element::Math { text, block: true } if text == "x^2")
        );
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_abbr_function() {
        let evaluated = Evaluator::default()
            .evaluate("#abbr(term=\"HTML\", expansion=\"HyperText Markup Language\")");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Abbr { term, expansion }
                if term == "HTML" && expansion == "HyperText Markup Language"
        ));
    }

    #[test]
    #[ignore = "legacy feature moved to plugin"]
    fn evaluates_cite_function_and_rejects_invalid_keys() {
        let evaluated = Evaluator::default().evaluate("#cite(\"doe2024\", \"p. 17\")");
        assert!(
            evaluated.diagnostics.is_empty(),
            "{:?}",
            evaluated.diagnostics
        );
        assert!(matches!(
            &evaluated.content.elements[0].element,
            Element::Citation { key, locator }
                if key == "doe2024" && locator.as_deref() == Some("p. 17")
        ));

        let invalid = Evaluator::default().evaluate("#cite(\"two words\")");
        assert!(invalid.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("citation key must be non-empty")
        }));
    }
}
