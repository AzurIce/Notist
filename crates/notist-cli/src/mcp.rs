use std::collections::BTreeSet;
use std::error::Error;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use notist_service::protocol::ClientKind;
use notist_service::{
    ByteRange, CoreRequest, CoreResponse, DiagnosticsQuery, EditOperation, ModulesQuery,
    OutlineQuery, PageRequest, ProtocolViewKind, ReadQuery, ReadWindow, ReferenceDirection,
    ReferencesQuery, SearchField, SearchGroup, SearchMode, SearchOperator, SearchQuery, Selector,
    ServiceViewId, SourceFingerprint, ToolError,
};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value, json};

use crate::service::LocalNotistClient;

const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: Some(json!({"retryable":false})),
        }
    }

    fn invalid_with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: Some(json!({"retryable":false,"hint":hint.into()})),
        }
    }

    fn unknown(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
            data: Some(json!({"retryable":false})),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: Some(json!({"retryable":true})),
        }
    }
}

pub fn run(
    root: PathBuf,
    no_daemon: bool,
    brief_text_mirror: bool,
) -> Result<ExitCode, Box<dyn Error>> {
    let mut client = LocalNotistClient::connect(no_daemon, ClientKind::Mcp, root.clone())?;
    let opened = client.request(CoreRequest::OpenView {
        root: root.clone(),
        kind: ProtocolViewKind::Disk,
    })?;
    let CoreResponse::Opened { view_id, vault } = opened.response else {
        return Err("service returned an unexpected open-view response".into());
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut stdout,
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error.to_string()}}),
                )?;
                continue;
            }
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            write_response(
                &mut stdout,
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"expected JSON-RPC 2.0 request"}}),
            )?;
            continue;
        }
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let response = match dispatch(
            &mut client,
            view_id,
            &root,
            &vault.fingerprint,
            method,
            params,
            brief_text_mirror,
        ) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => json!({"jsonrpc":"2.0","id":id,"error":{
                "code":error.code,"message":error.message,"data":error.data
            }}),
        };
        write_response(&mut stdout, response)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn dispatch(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    root: &Path,
    vault: &str,
    method: &str,
    params: Value,
    brief_text_mirror: bool,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities":{"tools":{"listChanged":false},"resources":{"subscribe":false,"listChanged":false}},
            "serverInfo":{"name":"notist","version":env!("CARGO_PKG_VERSION")}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => call_tool(client, view_id, root, params, brief_text_mirror),
        "resources/list" => Ok(json!({"resources":[
            {"uri":format!("notist://{vault}/summary"),"name":"Notist Vault summary","mimeType":"application/json"},
            {"uri":format!("notist://{vault}/capabilities"),"name":"Notist query capabilities","mimeType":"application/json"}
        ]})),
        "resources/templates/list" => Ok(json!({"resourceTemplates":[
            {"uriTemplate":format!("notist://{vault}/source/{{module}}?start={{start}}&end={{end}}&fp={{fingerprint}}"),"name":"Bounded Notist source range","mimeType":"text/plain"},
            {"uriTemplate":format!("notist://{vault}/module/{{module}}/metadata"),"name":"Notist module metadata","mimeType":"application/json"}
        ]})),
        "resources/read" => read_resource(client, view_id, vault, params),
        _ => Err(RpcError::unknown(format!(
            "unsupported MCP method `{method}`"
        ))),
    }
}

fn tool_definitions() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "vault_status",
            "Returns a small Vault, snapshot, diagnostics, and index summary.",
            object_schema(vec![]),
            false,
        ),
        tool(
            "list_modules",
            "Lists a small bounded page of ModulePath records for discovery. Continue only when the task needs more candidates or complete coverage.",
            paged_schema(
                vec![
                    ("prefix", json!({"type":"string","maxLength":4096})),
                    ("kind", json!({"enum":["any","source","virtual"]})),
                ],
                20,
            ),
            false,
        ),
        tool(
            "search",
            "Returns a small bounded candidate page. Lexical/fuzzy search groups by source by default; exact search returns individual matches. Keep operator=all for multi-term fact lookup and call read_source before using a hit as evidence. Continue only if this page is insufficient or the task requires exhaustive/negative coverage.",
            paged_schema(
                vec![
                    (
                        "query",
                        json!({"type":"string","minLength":1,"maxLength":4096}),
                    ),
                    (
                        "mode",
                        json!({"enum":["lexical","exact","fuzzy"],"default":"lexical"}),
                    ),
                    (
                        "scope",
                        json!({"type":"array","items":{"type":"string","maxLength":4096},"maxItems":32,"uniqueItems":true}),
                    ),
                    (
                        "fields",
                        json!({"type":"array","items":{"enum":["title","heading","label","module","path","tag","body","raw","comment"]},"maxItems":9,"uniqueItems":true}),
                    ),
                    ("operator", json!({"enum":["all","any"],"default":"all"})),
                    (
                        "groupBy",
                        json!({"enum":["source","section","match"],"description":"Result diversity: lexical/fuzzy default to source; exact defaults to match"}),
                    ),
                    (
                        "fuzzyDistance",
                        json!({"type":"integer","minimum":1,"maximum":2,"default":1}),
                    ),
                    (
                        "waitIndexMs",
                        json!({"type":"integer","minimum":0,"maximum":10000,"default":2000}),
                    ),
                    ("ignoreCase", json!({"type":"boolean","default":false})),
                    (
                        "snippetBytes",
                        json!({"type":"integer","minimum":64,"maximum":2048,"default":256}),
                    ),
                ],
                8,
            ),
            false,
        ),
        tool(
            "get_outline",
            "Returns the bounded heading tree for exactly one Module or source path. Pass selector as the module/path string returned by search or list_modules.",
            paged_schema(
                vec![
                    ("selector", selector_schema()),
                    (
                        "depth",
                        json!({"type":"integer","minimum":1,"maximum":6,"default":6}),
                    ),
                ],
                100,
            ),
            false,
        ),
        tool(
            "read_source",
            "Returns bounded authored source for evidence. Pass the module/path string returned by search or list_modules; continue only when the requested evidence extends beyond this page.",
            bounded_schema(vec![
                ("selector", selector_schema()),
                ("fromLine", json!({"type":"integer","minimum":1})),
                (
                    "lines",
                    json!({"type":"integer","minimum":1,"maximum":1000}),
                ),
                (
                    "byteRange",
                    json!({"type":"object","additionalProperties":false,"properties":{"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0}},"required":["start","end"]}),
                ),
            ]),
            false,
        ),
        tool(
            "get_references",
            "Returns bounded incoming or outgoing semantic edges. Pass selector as the module/path string returned by search or list_modules. Read source before using excerpts as evidence.",
            paged_schema(
                vec![
                    ("selector", selector_schema()),
                    (
                        "direction",
                        json!({"enum":["incoming","outgoing","both"],"default":"incoming"}),
                    ),
                    (
                        "includeDefinition",
                        json!({"type":"boolean","default":false}),
                    ),
                    (
                        "snippetBytes",
                        json!({"type":"integer","minimum":64,"maximum":2048,"default":256}),
                    ),
                ],
                20,
            ),
            false,
        ),
        tool(
            "get_definition",
            "Resolves a UTF-8 source byte offset to one definition location without returning full source.",
            object_schema(vec![
                (
                    "path",
                    json!({"type":"string","minLength":1,"maxLength":4096}),
                ),
                ("offset", json!({"type":"integer","minimum":0})),
                (
                    "expectedFingerprint",
                    json!({"type":"string","maxLength":128}),
                ),
            ]),
            false,
        ),
        tool(
            "check",
            "Analyzes the complete scope; diagnostic details may be paged while summary remains complete.",
            paged_schema(
                vec![
                    ("scope", json!({"type":"string","maxLength":4096})),
                    ("summaryOnly", json!({"type":"boolean","default":false})),
                    (
                        "severity",
                        json!({"enum":["error","warning","info"],"default":"error"}),
                    ),
                ],
                20,
            ),
            false,
        ),
        tool(
            "propose_edit",
            "Validates source edits and returns an edit plan; it does not write files.",
            object_schema(vec![
                ("baseRevision", json!({"type":"integer","minimum":0})),
                (
                    "operations",
                    json!({"type":"array","minItems":1,"maxItems":100,"items":{"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","maxLength":4096},"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0},"replacement":{"type":"string","maxLength":65536}},"required":["path","start","end","replacement"]}}),
                ),
            ]),
            false,
        ),
        tool(
            "apply_edit",
            "Applies an existing preconditioned edit plan. This operation is destructive and idempotent.",
            object_schema(vec![
                (
                    "planHash",
                    json!({"type":"string","minLength":1,"maxLength":128}),
                ),
                (
                    "expectedFingerprints",
                    json!({"type":"array","maxItems":100,"items":{"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","maxLength":4096},"fingerprint":{"type":"string","maxLength":128}},"required":["path","fingerprint"]}}),
                ),
                (
                    "idempotencyKey",
                    json!({"type":"string","minLength":1,"maxLength":256}),
                ),
            ]),
            true,
        ),
    ];
    for tool in &mut tools {
        let required: &[&str] = match tool.get("name").and_then(Value::as_str).unwrap_or("") {
            "search" => &["query"],
            "get_outline" | "read_source" | "get_references" => &["selector"],
            "get_definition" => &["path", "offset"],
            "propose_edit" => &["baseRevision", "operations"],
            "apply_edit" => &["planHash", "expectedFingerprints", "idempotencyKey"],
            _ => &[],
        };
        if !required.is_empty() {
            tool["inputSchema"]["required"] = json!(required);
        }
    }
    tools
}

fn tool(name: &str, description: &str, input_schema: Value, destructive: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "outputSchema": output_schema(name),
        "annotations": {
            "readOnlyHint": !destructive,
            "destructiveHint": destructive,
            "idempotentHint": name != "propose_edit",
            "openWorldHint": false
        }
    })
}

fn output_schema(name: &str) -> Value {
    let success = match name {
        "vault_status" => json!({
            "type":"object",
            "required":["root","source_count","module_count","diagnostic_count","runtime_mode","view_kind","snapshot","index"]
        }),
        "list_modules" | "search" | "get_outline" | "read_source" | "get_references" => {
            json!({"type":"object","required":["snapshot","items","page","budget","coverage"]})
        }
        "get_definition" => json!({
            "type":"object","additionalProperties":false,
            "properties":{"definition":{"oneOf":[{"type":"object"},{"type":"null"}]}},
            "required":["definition"]
        }),
        "check" => json!({"type":"object","required":["summary","diagnostics"]}),
        "propose_edit" => json!({
            "type":"object",
            "required":["plan_hash","base_revision","affected_sources","diagnostics","preview","preview_truncated"]
        }),
        "apply_edit" => json!({
            "type":"object",
            "required":["plan_hash","idempotency_key","resulting_fingerprints"]
        }),
        _ => json!({"type":"object"}),
    };
    let error = json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "code":{"type":"string"},
            "message":{"type":"string"},
            "retryable":{"type":"boolean"},
            "hint":{"type":"string"},
            "candidates":{"type":"array","items":{"type":"string"}}
        },
        "required":["code","message","retryable"]
    });
    json!({"oneOf":[success,error]})
}

fn object_schema(properties: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (name, schema) in properties {
        map.insert(name.into(), schema);
    }
    json!({"type":"object","additionalProperties":false,"properties":map})
}

fn paged_schema(mut properties: Vec<(&str, Value)>, default_limit: usize) -> Value {
    properties.push((
        "limit",
        json!({"type":"integer","minimum":1,"maximum":100,"default":default_limit}),
    ));
    bounded_schema(properties)
}

fn bounded_schema(mut properties: Vec<(&str, Value)>) -> Value {
    properties.extend([
        (
            "maxBytes",
            json!({"type":"integer","minimum":4096,"maximum":65536,"default":16384}),
        ),
        ("cursor", json!({"type":"string","maxLength":4096})),
    ]);
    object_schema(properties)
}

fn selector_schema() -> Value {
    json!({"oneOf":[
        {"type":"string","minLength":1,"maxLength":4096,"description":"Module, module#label, or Vault-relative .not path exactly as returned by Notist tools"},
        {"type":"object","additionalProperties":false,"properties":{"module":{"type":"string","minLength":1,"maxLength":4096,"pattern":"^vault(?:::)?.*$"},"label":{"type":"string","maxLength":4096}},"required":["module"]},
        {"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","maxLength":4096},"label":{"type":"string","maxLength":4096}},"required":["path"]}
    ]})
}

fn call_tool(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    root: &Path,
    params: Value,
    brief_text_mirror: bool,
) -> Result<Value, RpcError> {
    ensure_object_keys(&params, &["name", "arguments"])?;
    let name = string(&params, "name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = match name {
        "vault_status" => {
            ensure_object_keys(&arguments, &[])?;
            CoreRequest::Status { view_id }
        }
        "list_modules" => {
            ensure_object_keys(
                &arguments,
                &["prefix", "kind", "limit", "maxBytes", "cursor"],
            )?;
            let kind = match optional_string(&arguments, "kind").unwrap_or("any") {
                "any" => notist_service::ModuleKind::Any,
                "source" => notist_service::ModuleKind::Source,
                "virtual" => notist_service::ModuleKind::Virtual,
                _ => return Err(RpcError::invalid("kind must be any, source, or virtual")),
            };
            CoreRequest::ListModules {
                view_id,
                query: ModulesQuery {
                    prefix: optional_string(&arguments, "prefix").map(str::to_owned),
                    kind,
                    page: page(&arguments)?,
                },
            }
        }
        "search" => {
            ensure_object_keys(
                &arguments,
                &[
                    "query",
                    "mode",
                    "scope",
                    "fields",
                    "operator",
                    "groupBy",
                    "fuzzyDistance",
                    "waitIndexMs",
                    "ignoreCase",
                    "snippetBytes",
                    "limit",
                    "maxBytes",
                    "cursor",
                ],
            )?;
            let mode = match optional_string(&arguments, "mode").unwrap_or("lexical") {
                "lexical" => SearchMode::Lexical,
                "exact" => SearchMode::Exact,
                "fuzzy" => SearchMode::Fuzzy,
                _ => {
                    return Err(RpcError::invalid(
                        "MCP search mode must be lexical, exact, or fuzzy",
                    ));
                }
            };
            let operator = match optional_string(&arguments, "operator").unwrap_or("all") {
                "all" => SearchOperator::All,
                "any" => SearchOperator::Any,
                _ => return Err(RpcError::invalid("operator must be all or any")),
            };
            let group_by = optional_string(&arguments, "groupBy")
                .map(|group| match group {
                    "source" => Ok(SearchGroup::Source),
                    "section" => Ok(SearchGroup::Section),
                    "match" => Ok(SearchGroup::Match),
                    _ => Err(RpcError::invalid(
                        "groupBy must be source, section, or match",
                    )),
                })
                .transpose()?;
            let incompatible = match mode {
                SearchMode::Exact => ["operator", "fuzzyDistance", "waitIndexMs"].as_slice(),
                SearchMode::Lexical => ["fuzzyDistance", "ignoreCase"].as_slice(),
                SearchMode::Fuzzy => ["ignoreCase"].as_slice(),
                SearchMode::Regex => unreachable!("regex is not exposed through MCP"),
            };
            if let Some(name) = incompatible
                .iter()
                .find(|name| arguments.get(**name).is_some())
            {
                return Err(RpcError::invalid(format!(
                    "{name} is not valid in {mode:?} search mode"
                )));
            }
            CoreRequest::SearchPage {
                view_id,
                query: SearchQuery {
                    query: string(&arguments, "query")?.into(),
                    mode,
                    scopes: string_array(&arguments, "scope")?,
                    fields: search_fields(&arguments)?,
                    operator,
                    group_by,
                    ignore_case: boolean(&arguments, "ignoreCase", false)?,
                    fuzzy_distance: integer(&arguments, "fuzzyDistance", 1)? as u8,
                    wait_index_ms: integer(&arguments, "waitIndexMs", 2000)? as u64,
                    snippet_bytes: integer(&arguments, "snippetBytes", 256)?,
                    page: page(&arguments)?,
                },
            }
        }
        "get_outline" => {
            ensure_object_keys(
                &arguments,
                &["selector", "depth", "limit", "maxBytes", "cursor"],
            )?;
            CoreRequest::OutlineModule {
                view_id,
                query: OutlineQuery {
                    selector: selector(&arguments)?,
                    depth: integer(&arguments, "depth", 6)? as u8,
                    page: page(&arguments)?,
                },
            }
        }
        "read_source" => {
            ensure_object_keys(
                &arguments,
                &[
                    "selector",
                    "fromLine",
                    "lines",
                    "byteRange",
                    "maxBytes",
                    "cursor",
                ],
            )?;
            let byte_range = arguments.get("byteRange").map(byte_range).transpose()?;
            CoreRequest::ReadSource {
                view_id,
                query: ReadQuery {
                    selector: selector(&arguments)?,
                    window: ReadWindow {
                        from_line: optional_integer(&arguments, "fromLine")?,
                        lines: optional_integer(&arguments, "lines")?,
                        byte_range,
                    },
                    page: page(&arguments)?,
                },
            }
        }
        "get_references" => {
            ensure_object_keys(
                &arguments,
                &[
                    "selector",
                    "direction",
                    "includeDefinition",
                    "snippetBytes",
                    "limit",
                    "maxBytes",
                    "cursor",
                ],
            )?;
            let direction = match optional_string(&arguments, "direction").unwrap_or("incoming") {
                "incoming" => ReferenceDirection::Incoming,
                "outgoing" => ReferenceDirection::Outgoing,
                "both" => ReferenceDirection::Both,
                _ => {
                    return Err(RpcError::invalid(
                        "direction must be incoming, outgoing, or both",
                    ));
                }
            };
            CoreRequest::ReferencesPage {
                view_id,
                query: ReferencesQuery {
                    selector: selector(&arguments)?,
                    direction,
                    include_definition: boolean(&arguments, "includeDefinition", false)?,
                    snippet_bytes: integer(&arguments, "snippetBytes", 256)?,
                    page: page(&arguments)?,
                },
            }
        }
        "get_definition" => {
            ensure_object_keys(&arguments, &["path", "offset", "expectedFingerprint"])?;
            CoreRequest::DefinitionLocation {
                view_id,
                query: notist_service::DefinitionQuery {
                    path: resolve_source_path(root, string(&arguments, "path")?)?,
                    offset: integer(&arguments, "offset", 0)?,
                    expected_fingerprint: optional_string(&arguments, "expectedFingerprint")
                        .map(str::to_owned),
                },
            }
        }
        "check" => {
            ensure_object_keys(
                &arguments,
                &[
                    "scope",
                    "summaryOnly",
                    "severity",
                    "limit",
                    "maxBytes",
                    "cursor",
                ],
            )?;
            CoreRequest::DiagnosticsPage {
                view_id,
                query: DiagnosticsQuery {
                    scope: optional_string(&arguments, "scope").map(str::to_owned),
                    summary_only: boolean(&arguments, "summaryOnly", false)?,
                    severity: match optional_string(&arguments, "severity").unwrap_or("error") {
                        "error" => notist_service::DiagnosticSeverity::Error,
                        "warning" => notist_service::DiagnosticSeverity::Warning,
                        "info" => notist_service::DiagnosticSeverity::Info,
                        _ => {
                            return Err(RpcError::invalid(
                                "severity must be error, warning, or info",
                            ));
                        }
                    },
                    page: page(&arguments)?,
                },
            }
        }
        "propose_edit" => {
            ensure_object_keys(&arguments, &["baseRevision", "operations"])?;
            CoreRequest::ProposeEdit {
                view_id,
                base_revision: integer(&arguments, "baseRevision", 0)? as u64,
                operations: operations(root, &arguments)?,
            }
        }
        "apply_edit" => {
            ensure_object_keys(
                &arguments,
                &["planHash", "expectedFingerprints", "idempotencyKey"],
            )?;
            CoreRequest::ApplyEdit {
                view_id,
                plan_hash: string(&arguments, "planHash")?.into(),
                expected_fingerprints: fingerprints(root, &arguments)?,
                idempotency_key: string(&arguments, "idempotencyKey")?.into(),
            }
        }
        _ => return Err(RpcError::unknown(format!("unknown tool `{name}`"))),
    };
    match client.request(request) {
        Ok(response) => tool_response(response.response, response.snapshot, brief_text_mirror),
        Err(error) => {
            let code = match error.kind() {
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::AlreadyExists => {
                    "edit_conflict"
                }
                std::io::ErrorKind::NotFound => "plan_expired",
                std::io::ErrorKind::InvalidInput => "invalid_argument",
                _ => "service_unavailable",
            };
            let typed = ToolError {
                code: code.into(),
                message: error.to_string(),
                retryable: true,
                hint: Some("refresh the relevant source or plan state and retry".into()),
                candidates: Vec::new(),
            };
            let logical = serde_json::to_value(&typed)
                .map_err(|error| RpcError::internal(error.to_string()))?;
            let text = serde_json::to_string(&logical)
                .map_err(|error| RpcError::internal(error.to_string()))?;
            Ok(
                json!({"content":[{"type":"text","text":text}],"structuredContent":logical,"isError":true}),
            )
        }
    }
}

fn tool_response(
    response: CoreResponse,
    snapshot: notist_service::SnapshotIdentity,
    brief_text_mirror: bool,
) -> Result<Value, RpcError> {
    if let CoreResponse::QueryError(error) = response {
        let logical =
            serde_json::to_value(&error).map_err(|error| RpcError::internal(error.to_string()))?;
        let text = serde_json::to_string(&logical)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        return Ok(
            json!({"content":[{"type":"text","text":text}],"structuredContent":logical,"isError":true,"_meta":{"snapshot":snapshot}}),
        );
    }
    let logical = match response {
        CoreResponse::Status(value) => serde_json::to_value(value),
        CoreResponse::Modules(value) => serde_json::to_value(value),
        CoreResponse::SearchPage(value) => serde_json::to_value(value),
        CoreResponse::OutlinePage(value) => serde_json::to_value(value),
        CoreResponse::SourcePage(value) => serde_json::to_value(value),
        CoreResponse::ReferencesPage(value) => serde_json::to_value(value),
        CoreResponse::DefinitionLocation(value) => {
            serde_json::to_value(json!({"definition":value}))
        }
        CoreResponse::DiagnosticsPage(value) => serde_json::to_value(value),
        CoreResponse::EditPlan(value) => serde_json::to_value(value),
        CoreResponse::EditApplied(value) => serde_json::to_value(value),
        _ => {
            return Err(RpcError::internal(
                "service returned an unexpected tool response",
            ));
        }
    }
    .map_err(|error| RpcError::internal(error.to_string()))?;
    let text = if brief_text_mirror {
        brief_tool_text(&logical)
    } else {
        serde_json::to_string(&logical).map_err(|error| RpcError::internal(error.to_string()))?
    };
    Ok(
        json!({"content":[{"type":"text","text":text}],"structuredContent":logical,"isError":false,"_meta":{"snapshot":snapshot}}),
    )
}

fn brief_tool_text(logical: &Value) -> String {
    let page = logical
        .get("page")
        .or_else(|| logical.pointer("/diagnostics/page"));
    if let Some(page) = page {
        let returned = page.get("returned").and_then(Value::as_u64).unwrap_or(0);
        let has_more = page
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let complete = logical
            .get("coverage")
            .or_else(|| logical.pointer("/diagnostics/coverage"))
            .and_then(|coverage| coverage.get("complete"))
            .and_then(Value::as_bool)
            .unwrap_or(!has_more);
        return format!(
            "Notist returned {returned} structured item(s); coverage is {}. Inspect structuredContent{}.",
            if complete { "complete" } else { "incomplete" },
            if has_more { " and next_cursor" } else { "" }
        );
    }
    "Notist returned a structured result; inspect structuredContent.".into()
}

fn read_resource(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    vault: &str,
    params: Value,
) -> Result<Value, RpcError> {
    ensure_object_keys(&params, &["uri"])?;
    let uri = string(&params, "uri")?;
    if uri == format!("notist://{vault}/capabilities") {
        let value = json!({
            "schemaVersion":2,
            "ordinaryQuery":{"defaultMaxBytes":16384,"hardMaxBytes":65536},
            "mcpTransport":{"formula":"3 * appliedMaxBytes + 16384","defaultMaximum":65536,"hardMaximum":212992,"textMirrorModes":["full","brief"]},
            "searchModes":["lexical","exact","fuzzy"]
        });
        return resource_content(uri, value);
    }
    if uri == format!("notist://{vault}/summary") {
        let response = client
            .request(CoreRequest::Status { view_id })
            .map_err(|error| RpcError::internal(error.to_string()))?;
        let CoreResponse::Status(status) = response.response else {
            return Err(RpcError::internal("unexpected status resource response"));
        };
        return resource_content(
            uri,
            serde_json::to_value(status).map_err(|error| RpcError::internal(error.to_string()))?,
        );
    }
    let source_prefix = format!("notist://{vault}/source/");
    if let Some(target) = uri.strip_prefix(&source_prefix) {
        let (module, query) = target
            .split_once('?')
            .ok_or_else(|| RpcError::invalid("source resource requires start, end, and fp"))?;
        let module = percent_decode_str(module)
            .decode_utf8()
            .map_err(|_| RpcError::invalid("source module is not valid UTF-8"))?
            .into_owned();
        let parameters = query
            .split('&')
            .filter_map(|item| item.split_once('='))
            .collect::<std::collections::HashMap<_, _>>();
        let start = parameters
            .get("start")
            .ok_or_else(|| RpcError::invalid("source resource is missing start"))?
            .parse::<usize>()
            .map_err(|_| RpcError::invalid("source start is invalid"))?;
        let end = parameters
            .get("end")
            .ok_or_else(|| RpcError::invalid("source resource is missing end"))?
            .parse::<usize>()
            .map_err(|_| RpcError::invalid("source end is invalid"))?;
        let expected = parameters
            .get("fp")
            .ok_or_else(|| RpcError::invalid("source resource is missing fp"))?;
        if start > end || end.saturating_sub(start) > 60 * 1024 {
            return Err(RpcError::invalid(
                "source resource range must be ordered and at most 60 KiB",
            ));
        }
        let response = client
            .request(CoreRequest::ReadSource {
                view_id,
                query: ReadQuery {
                    selector: Selector::Module {
                        module,
                        label: None,
                    },
                    window: ReadWindow {
                        from_line: None,
                        lines: None,
                        byte_range: Some(ByteRange { start, end }),
                    },
                    page: PageRequest {
                        limit: Some(1),
                        max_bytes: Some((end.saturating_sub(start) + 4096).clamp(4096, 65536)),
                        cursor: None,
                    },
                },
            })
            .map_err(|error| RpcError::internal(error.to_string()))?;
        let CoreResponse::SourcePage(page) = response.response else {
            return Err(RpcError::internal("unexpected source resource response"));
        };
        let chunk = page
            .items
            .first()
            .ok_or_else(|| RpcError::invalid("source range returned no content"))?;
        if chunk.location.source_fingerprint != *expected {
            return Err(RpcError::invalid("source resource fingerprint is stale"));
        }
        return Ok(json!({"contents":[{"uri":uri,"mimeType":"text/plain","text":chunk.source}]}));
    }
    let module_prefix = format!("notist://{vault}/module/");
    if let Some(target) = uri
        .strip_prefix(&module_prefix)
        .and_then(|target| target.strip_suffix("/metadata"))
    {
        let module = percent_decode_str(target)
            .decode_utf8()
            .map_err(|_| RpcError::invalid("module resource is not valid UTF-8"))?
            .into_owned();
        let response = client
            .request(CoreRequest::ListModules {
                view_id,
                query: ModulesQuery {
                    prefix: Some(module.clone()),
                    kind: notist_service::ModuleKind::Any,
                    page: PageRequest {
                        limit: Some(100),
                        max_bytes: Some(32768),
                        cursor: None,
                    },
                },
            })
            .map_err(|error| RpcError::internal(error.to_string()))?;
        let CoreResponse::Modules(page) = response.response else {
            return Err(RpcError::internal("unexpected module resource response"));
        };
        let item = page
            .items
            .into_iter()
            .find(|item| item.module == module)
            .ok_or_else(|| RpcError::invalid("module resource was not found"))?;
        return resource_content(
            uri,
            serde_json::to_value(item).map_err(|error| RpcError::internal(error.to_string()))?,
        );
    }
    Err(RpcError::invalid(format!("unknown resource `{uri}`")))
}

fn resource_content(uri: &str, value: Value) -> Result<Value, RpcError> {
    let text =
        serde_json::to_string(&value).map_err(|error| RpcError::internal(error.to_string()))?;
    Ok(json!({"contents":[{"uri":uri,"mimeType":"application/json","text":text}]}))
}

fn ensure_object_keys(value: &Value, allowed: &[&str]) -> Result<(), RpcError> {
    let object = value
        .as_object()
        .ok_or_else(|| RpcError::invalid("expected object parameters"))?;
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if let Some(key) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(RpcError::invalid(format!("unknown parameter `{key}`")));
    }
    Ok(())
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, RpcError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid(format!("missing string parameter `{name}`")))
}

fn optional_string<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

fn integer(value: &Value, name: &str, default: usize) -> Result<usize, RpcError> {
    let raw = value
        .get(name)
        .map_or(default as u64, |value| value.as_u64().unwrap_or(u64::MAX));
    usize::try_from(raw)
        .map_err(|_| RpcError::invalid(format!("`{name}` must be a non-negative integer")))
}

fn optional_integer(value: &Value, name: &str) -> Result<Option<usize>, RpcError> {
    value.get(name).map(|_| integer(value, name, 0)).transpose()
}

fn boolean(value: &Value, name: &str, default: bool) -> Result<bool, RpcError> {
    value.get(name).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| RpcError::invalid(format!("`{name}` must be boolean")))
    })
}

fn page(arguments: &Value) -> Result<PageRequest, RpcError> {
    Ok(PageRequest {
        limit: optional_integer(arguments, "limit")?,
        max_bytes: optional_integer(arguments, "maxBytes")?,
        cursor: optional_string(arguments, "cursor").map(str::to_owned),
    })
}

fn selector(arguments: &Value) -> Result<Selector, RpcError> {
    let value = arguments
        .get("selector")
        .ok_or_else(|| RpcError::invalid("missing required parameter `selector`"))?;
    if let Some(value) = value.as_str() {
        if value.is_empty() || value.len() > 4096 {
            return Err(RpcError::invalid(
                "`selector` string must contain 1 to 4096 UTF-8 bytes",
            ));
        }
        return Ok(Selector::parse(value));
    }
    if !value.is_object() {
        return Err(RpcError::invalid_with_hint(
            format!(
                "`selector` must be a module/path string or an object {{\"module\":\"vault::…\"}} or {{\"path\":\"…not\"}}, got {}",
                json_type(value)
            ),
            "copy the `module` string from search/list_modules directly into `selector`",
        ));
    }
    ensure_object_keys(value, &["module", "path", "label"])?;
    let label = optional_string(value, "label").map(str::to_owned);
    match (
        optional_string(value, "module"),
        optional_string(value, "path"),
    ) {
        (Some(module), None) => Ok(Selector::Module {
            module: module.into(),
            label,
        }),
        (None, Some(path)) => Ok(Selector::Path {
            path: path.into(),
            label,
        }),
        _ => Err(RpcError::invalid(
            "`selector` object must contain exactly one string property: `module` or `path`",
        )),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn byte_range(value: &Value) -> Result<ByteRange, RpcError> {
    ensure_object_keys(value, &["start", "end"])?;
    let start = integer(value, "start", 0)?;
    let end = integer(value, "end", 0)?;
    if start > end {
        return Err(RpcError::invalid("byte range start exceeds end"));
    }
    Ok(ByteRange { start, end })
}

fn string_array(value: &Value, name: &str) -> Result<Vec<String>, RpcError> {
    let Some(array) = value.get(name) else {
        return Ok(Vec::new());
    };
    let array = array
        .as_array()
        .ok_or_else(|| RpcError::invalid(format!("`{name}` must be an array")))?;
    if array.len() > 32 {
        return Err(RpcError::invalid(format!("`{name}` has too many values")));
    }
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| RpcError::invalid(format!("`{name}` values must be strings")))
        })
        .collect()
}

fn search_fields(value: &Value) -> Result<Vec<SearchField>, RpcError> {
    let fields = string_array(value, "fields")?;
    if fields.is_empty() {
        return Ok(SearchField::defaults());
    }
    fields
        .into_iter()
        .map(|field| {
            Ok(match field.as_str() {
                "title" => SearchField::Title,
                "heading" => SearchField::Heading,
                "label" => SearchField::Label,
                "module" => SearchField::Module,
                "path" => SearchField::Path,
                "tag" => SearchField::Tag,
                "body" => SearchField::Body,
                "raw" => SearchField::Raw,
                "comment" => SearchField::Comment,
                _ => return Err(RpcError::invalid(format!("unknown search field `{field}`"))),
            })
        })
        .collect()
}

fn resolve_source_path(root: &Path, value: &str) -> Result<PathBuf, RpcError> {
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let path = dunce::canonicalize(path).map_err(|error| RpcError::invalid(error.to_string()))?;
    if !path.starts_with(root) {
        return Err(RpcError::invalid("source path escapes the Vault"));
    }
    Ok(path)
}

fn operations(root: &Path, arguments: &Value) -> Result<Vec<EditOperation>, RpcError> {
    let values = arguments
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid("missing operations array"))?;
    if values.is_empty() || values.len() > 100 {
        return Err(RpcError::invalid("operations must contain 1 to 100 edits"));
    }
    values
        .iter()
        .map(|value| {
            ensure_object_keys(value, &["path", "start", "end", "replacement"])?;
            let range = ByteRange {
                start: integer(value, "start", 0)?,
                end: integer(value, "end", 0)?,
            };
            if range.start > range.end {
                return Err(RpcError::invalid("edit start exceeds end"));
            }
            let replacement = string(value, "replacement")?;
            if replacement.len() > 65536 {
                return Err(RpcError::invalid("replacement exceeds 64 KiB"));
            }
            Ok(EditOperation {
                path: resolve_source_path(root, string(value, "path")?)?,
                range,
                replacement: replacement.into(),
            })
        })
        .collect()
}

fn fingerprints(root: &Path, arguments: &Value) -> Result<Vec<SourceFingerprint>, RpcError> {
    let values = arguments
        .get("expectedFingerprints")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid("missing expectedFingerprints array"))?;
    if values.len() > 100 {
        return Err(RpcError::invalid("too many expected fingerprints"));
    }
    values
        .iter()
        .map(|value| {
            ensure_object_keys(value, &["path", "fingerprint"])?;
            Ok(SourceFingerprint {
                path: resolve_source_path(root, string(value, "path")?)?,
                fingerprint: string(value, "fingerprint")?.into(),
            })
        })
        .collect()
}

fn write_response(output: &mut impl Write, response: Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *output, &response)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn mcp_surface_is_bounded_and_strict() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("Notist.toml"), "").unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Searchable\n\nsearchable text",
        )
        .unwrap();
        let mut client =
            LocalNotistClient::connect(true, ClientKind::Mcp, root.path().to_path_buf()).unwrap();
        let opened = client
            .request(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, vault } = opened.response else {
            panic!("expected open view")
        };
        let tools = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/list",
            json!({}),
            false,
        )
        .unwrap();
        assert!(tools.to_string().contains("read_source"));
        assert!(!tools.to_string().contains("regex"));
        let search_tool = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "search")
            .unwrap();
        assert_eq!(
            search_tool["inputSchema"]["properties"]["limit"]["default"],
            8
        );
        assert_eq!(
            search_tool["inputSchema"]["properties"]["snippetBytes"]["default"],
            256
        );
        assert!(
            search_tool["inputSchema"]["properties"]
                .get("groupBy")
                .is_some()
        );
        let read_tool = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "read_source")
            .unwrap();
        assert_eq!(
            read_tool["inputSchema"]["properties"]["selector"]["oneOf"][0]["type"],
            "string"
        );
        assert!(search_tool["outputSchema"].to_string().contains("coverage"));
        let search = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/call",
            json!({"name":"search","arguments":{"query":"searchable","limit":1}}),
            false,
        )
        .unwrap();
        assert!(search.to_string().contains("searchable"));
        assert!(
            serde_json::to_vec(&search["structuredContent"])
                .unwrap()
                .len()
                <= 16 * 1024
        );
        assert!(serde_json::to_vec(&search).unwrap().len() <= 3 * 16 * 1024 + 16 * 1024);
        let brief = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/call",
            json!({"name":"search","arguments":{"query":"searchable","limit":1}}),
            true,
        )
        .unwrap();
        assert!(
            brief["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("structuredContent")
        );
        assert!(
            brief["content"][0]["text"].as_str().unwrap().len()
                < search["content"][0]["text"].as_str().unwrap().len()
        );
        let module = search["structuredContent"]["items"][0]["location"]["module"]
            .as_str()
            .unwrap();
        for name in ["read_source", "get_outline", "get_references"] {
            let response = dispatch(
                &mut client,
                view_id,
                root.path(),
                &vault.fingerprint,
                "tools/call",
                json!({"name":name,"arguments":{"selector":module}}),
                false,
            )
            .unwrap();
            assert_eq!(response["isError"], false, "{name}: {response}");
        }
        let selector_error = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/call",
            json!({"name":"read_source","arguments":{"selector":true}}),
            false,
        )
        .unwrap_err();
        assert!(selector_error.message.contains("`selector` must be"));
        assert_eq!(selector_error.data.unwrap()["retryable"], false);
        let error = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/call",
            json!({"name":"search","arguments":{"query":"x","limti":1}}),
            false,
        )
        .unwrap_err();
        assert_eq!(error.code, -32602);
        let incompatible = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "tools/call",
            json!({"name":"search","arguments":{"query":"x","mode":"exact","waitIndexMs":1}}),
            false,
        )
        .unwrap_err();
        assert_eq!(incompatible.code, -32602);
        let resources = dispatch(
            &mut client,
            view_id,
            root.path(),
            &vault.fingerprint,
            "resources/list",
            json!({}),
            false,
        )
        .unwrap();
        assert!(resources.to_string().contains("/summary"));
        assert!(!resources.to_string().contains("/diagnostics"));
    }
}
