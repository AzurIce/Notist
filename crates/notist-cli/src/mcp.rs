use std::error::Error;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind, ServiceViewId};
use serde_json::{Value, json};

use crate::service::LocalNotistClient;

pub fn run(root: PathBuf, no_daemon: bool) -> Result<ExitCode, Box<dyn Error>> {
    let mut client = LocalNotistClient::connect(no_daemon, ClientKind::Mcp)?;
    let opened = client.request(CoreRequest::OpenView {
        root,
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
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let result = dispatch(&mut client, view_id, &vault.fingerprint, method, params);
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => json!({
                "jsonrpc":"2.0",
                "id":id,
                "error":{"code":-32602,"message":error.to_string()}
            }),
        };
        write_response(&mut stdout, response)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn dispatch(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    vault: &str,
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion":"2025-06-18",
            "capabilities":{"tools":{},"resources":{}},
            "serverInfo":{"name":"notist","version":env!("CARGO_PKG_VERSION")}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":[
            {
                "name":"search",
                "description":"Search captured Notist source context in the disk view.",
                "inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}
            },
            {
                "name":"get_references",
                "description":"Find semantic references at a source byte offset.",
                "inputSchema":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"include_definition":{"type":"boolean"}},"required":["path","offset"]}
            },
            {
                "name":"definition",
                "description":"Resolve a semantic definition at a source byte offset.",
                "inputSchema":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0}},"required":["path","offset"]}
            }
        ]})),
        "tools/call" => call_tool(client, view_id, params),
        "resources/list" => Ok(json!({"resources":[
            {"uri":format!("notist://{vault}/diagnostics"),"name":"Notist diagnostics","mimeType":"application/json"},
            {"uri":format!("notist://{vault}/outline"),"name":"Notist outline","mimeType":"application/json"}
        ]})),
        "resources/read" => read_resource(client, view_id, vault, params),
        _ => Err(format!("unsupported MCP method `{method}`").into()),
    }
}

fn call_tool(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing tool name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let response = match name {
        "search" => client.request(CoreRequest::Search {
            view_id,
            query: string_argument(&arguments, "query")?.into(),
        })?,
        "get_references" => client.request(CoreRequest::References {
            view_id,
            path: canonical_path_argument(&arguments, "path")?,
            offset: usize_argument(&arguments, "offset")?,
            include_definition: arguments
                .get("include_definition")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })?,
        "definition" => client.request(CoreRequest::Definition {
            view_id,
            path: canonical_path_argument(&arguments, "path")?,
            offset: usize_argument(&arguments, "offset")?,
        })?,
        _ => return Err(format!("unknown tool `{name}`").into()),
    };
    Ok(json!({
        "content":[{"type":"text","text":serde_json::to_string_pretty(&response.response)?}],
        "structuredContent":response.response,
        "_meta":{"snapshot":response.snapshot}
    }))
}

fn read_resource(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    vault: &str,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or("missing resource URI")?;
    let response = if uri == format!("notist://{vault}/diagnostics") {
        client.request(CoreRequest::Diagnostics { view_id })?
    } else if uri == format!("notist://{vault}/outline") {
        client.request(CoreRequest::Outline { view_id })?
    } else {
        return Err(format!("unknown resource `{uri}`").into());
    };
    Ok(json!({"contents":[{
        "uri":uri,
        "mimeType":"application/json",
        "text":serde_json::to_string_pretty(&response.response)?
    }]}))
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, Box<dyn Error>> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument `{name}`").into())
}

fn usize_argument(arguments: &Value, name: &str) -> Result<usize, Box<dyn Error>> {
    let value = arguments
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing non-negative integer argument `{name}`"))?;
    usize::try_from(value).map_err(Into::into)
}

fn canonical_path_argument(arguments: &Value, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(dunce::canonicalize(string_argument(arguments, name)?)?)
}

fn write_response(output: &mut impl Write, response: Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *output, &response)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn mcp_tools_and_resources_use_core_requests() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "searchable text").unwrap();
        let mut client = LocalNotistClient::connect(true, ClientKind::Mcp).unwrap();
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
            &vault.fingerprint,
            "tools/list",
            json!({}),
        )
        .unwrap();
        assert!(tools.to_string().contains("get_references"));
        let search = dispatch(
            &mut client,
            view_id,
            &vault.fingerprint,
            "tools/call",
            json!({"name":"search","arguments":{"query":"searchable"}}),
        )
        .unwrap();
        assert!(search.to_string().contains("searchable text"));
        let resources = dispatch(
            &mut client,
            view_id,
            &vault.fingerprint,
            "resources/list",
            json!({}),
        )
        .unwrap();
        assert!(resources.to_string().contains("/diagnostics"));
    }
}
