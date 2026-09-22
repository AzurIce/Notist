//! JSON projection for the CLI and browser debugger, separate from input snapshots.
use crate::EvaluationSession;
use base64::Engine;
use notist_syntax::{Expr, ExprKind, Statement};
use serde_json::{Value as Json, json};

pub fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|e| e.to_string())
}

fn expr(e: &Expr) -> Json {
    let mut node = match &e.kind {
        ExprKind::Element(name, fields) => {
            json!({"kind":"element","name":name,"fields":fields.iter().map(|(k,v)| (k,expr(v))).collect::<std::collections::BTreeMap<_,_>>()})
        }
        ExprKind::Annotation(module, value) => {
            json!({"kind":"annotation","module":module,"value":expr(value)})
        }
        ExprKind::Typed(ty, value) => {
            json!({"kind":"typed","type":format!("{ty:?}"),"value":expr(value)})
        }
        ExprKind::None => json!({"kind":"none"}),
        ExprKind::String(v) => json!({"kind":"string","value":v}),
        ExprKind::Int(v) => json!({"kind":"int","value":v}),
        ExprKind::Bool(v) => json!({"kind":"bool","value":v}),
        ExprKind::Name(n) => json!({"kind":"name","name":n}),
        ExprKind::Section(level, title, body) => {
            json!({"kind":"section","level":level,"title":title.iter().map(expr).collect::<Vec<_>>(),"body":body.iter().map(expr).collect::<Vec<_>>()})
        }
        ExprKind::Declaration(s) => json!({"kind":"declaration","statement":format!("{s:?}")}),
        ExprKind::Target(path, labels) => json!({"kind":"target","module":path,"labels":labels}),
        ExprKind::List(v) => json!({"kind":"list","items":v.iter().map(expr).collect::<Vec<_>>()}),
        ExprKind::Dict(v) => {
            json!({"kind":"dict","fields":v.iter().map(|(k,v)|json!({"key":k,"value":expr(v)})).collect::<Vec<_>>()})
        }
        ExprKind::Content(v) => {
            json!({"kind":"content","parts":v.iter().map(expr).collect::<Vec<_>>()})
        }
        ExprKind::Styled(s, v) => {
            json!({"kind":"styled","style":s,"parts":v.iter().map(expr).collect::<Vec<_>>()})
        }
        ExprKind::Field(v, n) => json!({"kind":"field","base":expr(v),"field":n}),
        ExprKind::Lambda(p, b) => {
            json!({"kind":"lambda","params":p.iter().map(|p|json!({"name":p.name,"type":format!("{:?}",p.ty),"default":p.default.as_ref().map(expr)})).collect::<Vec<_>>(),"body":expr(b)})
        }
        ExprKind::Call(f, a) => {
            json!({"kind":"call","callee":expr(f),"args":a.iter().map(|a|json!({"name":a.name,"trailing":a.trailing,"expr":expr(&a.expr)})).collect::<Vec<_>>()})
        }
        ExprKind::If(c, y, n) => {
            json!({"kind":"if","condition":expr(c),"then":expr(y),"else":expr(n)})
        }
        ExprKind::Binary(op, a, b) => {
            json!({"kind":"binary","op":op,"left":expr(a),"right":expr(b)})
        }
    };
    node["id"] = json!(e.id);
    node["range"] = json!([e.offset, e.end]);
    node
}
pub fn analyze(input: &str) -> Json {
    let started = web_time::Instant::now();
    let mut failures = Vec::new();
    let request: Json = serde_json::from_str(input).unwrap_or_else(|e| {
        failures.push(e.to_string());
        json!({})
    });
    let entry = request["entry"].as_str().unwrap_or("main.notc");
    let mut runtime = EvaluationSession::default();
    if let Some(files) = request["files"].as_object() {
        for (path, text) in files {
            runtime
                .sources
                .insert(path.clone(), text.as_str().unwrap_or_default().into());
        }
    }
    if let Some(binaries) = request["binaries"].as_object() {
        for (path, value) in binaries {
            match base64_decode(value.as_str().unwrap_or("")) {
                Ok(bytes) => {
                    runtime.binaries.insert(path.clone(), bytes);
                }
                Err(e) => failures.push(e),
            }
        }
    }
    if let Some(deps) = request.get("dependencies") {
        match serde_json::from_value(deps.clone()) {
            Ok(d) => runtime.dependencies = d,
            Err(e) => failures.push(e.to_string()),
        }
    }
    let input = runtime.snapshot();
    let files = input
        .sources()
        .iter()
        .enumerate()
        .map(|(id, (path, source))| json!({"source_id":id,"path":path,"text":source.text.as_ref()}))
        .collect::<Vec<_>>();
    let source_id = |path: &str| files.iter().position(|f| f["path"] == path).unwrap_or(0);
    let mut tokens = Vec::new();
    let mut statements = Vec::new();
    let mut errors = Vec::new();
    let mut id_offset = 0;
    fn remap(node: &mut Json, offset: usize) {
        match node {
            Json::Object(fields) => {
                if let Some(id) = fields.get_mut("id")
                    && let Some(n) = id.as_u64()
                {
                    *id = json!(n + offset as u64);
                }
                for value in fields.values_mut() {
                    remap(value, offset);
                }
            }
            Json::Array(values) => {
                for value in values {
                    remap(value, offset);
                }
            }
            _ => {}
        }
    }
    for (path, source) in input.sources() {
        let parsed = &source.parsed;
        let sid = source_id(path);
        for t in &parsed.tokens {
            tokens.push(json!({"i":t.i,"source_id":sid,"range":[t.start,t.end],"kind":t.kind,"text":t.text,"recovery":t.recovery}));
        }
        for ((s, r), id) in parsed
            .statements
            .iter()
            .zip(&parsed.stmt_ranges)
            .zip(&parsed.stmt_ids)
        {
            let mut node = match s {
                Statement::Let(n, e) => json!({"kind":"let","name":n,"expr":expr(e)}),
                Statement::Expression(e) => json!({"kind":"expression","expr":expr(e)}),
                Statement::Wasm(p) => json!({"kind":"wasm","path":p}),
                Statement::Use(paths) => {
                    json!({"kind":"use","imports":paths.iter().map(|p|json!({"path":p.path,"alias":p.alias,"glob":p.glob})).collect::<Vec<_>>()})
                }
                Statement::Error(_, m) => json!({"kind":"error","message":m}),
            };
            node["id"] = json!(id);
            node["source_id"] = json!(sid);
            node["range"] = json!([r.0, r.1]);
            remap(&mut node, id_offset);
            statements.push(node);
        }
        for e in &parsed.errors {
            errors.push(json!({"source_id":sid,"range":[e.start,e.end],"message":e.message}));
        }
        id_offset += parsed.id_count;
    }
    let mut runtime = notist_eval::Runtime::new(&input).with_call_trace();
    let (evaluation, env) = runtime.evaluate_with_env(entry);
    let diagnostics=failures.iter().map(|e|json!({"severity":"failure","stage":"setup","source_id":source_id(entry),"range":[0,0],"message":e}))
        .chain(evaluation.diagnostics.iter().map(|d| {
            let mut d = d.clone(); input.enrich(&mut d);
            let source = d.location.as_ref().map_or(entry, |l| l.source.as_str());
            let range = d.span.as_ref().map_or([0,0], |s| [s.start,s.end]);
            json!({"severity":"error","stage":if d.code == notist_model::DiagnosticCode::ContentConstraint { "form" } else { "evaluate" },"code":d.code.as_str(),"source_id":source_id(source),"range":range,"message":d.message})
        })).collect::<Vec<_>>();
    let events=runtime.events.iter().enumerate().map(|(i,e)|json!({"i":i,"phase":"evaluate","kind":e["kind"],"node":null,"source_id":source_id(e["source"].as_str().unwrap_or(entry)),"range":e["range"],"name":null,"detail":"function evaluation","in":null,"out":e["out"].to_string()})).collect::<Vec<_>>();
    json!({"protocol":"notist-debug-snapshot","request_id":request["request_id"].as_str().unwrap_or("native"),
        "platform":{"target":if cfg!(target_arch="wasm32"){"wasm32-unknown-unknown"}else{"native"},"elapsed_ms":started.elapsed().as_secs_f64()*1000.0},
        "source":{"files":files,"entry_source_id":source_id(entry)},"syntax":{"tokens":tokens,"statements":statements,"errors":errors},
        "evaluation":{"events":events,"content":evaluation.raw_content.to_json()},"result":{"content":evaluation.content.to_json(),"bindings":env.iter().map(|(k,v)|json!({"name":k,"value":v.to_json().to_string(),"source_id":source_id(entry)})).collect::<Vec<_>>(),
        "attributes":evaluation.attributes.iter().map(|(k,v)|(k,v.to_json())).collect::<std::collections::BTreeMap<_,_>>(),
        "modules":runtime.module_attributes.iter().map(|(source,attrs)|(source,attrs.iter().map(|(k,v)|(k,v.to_json())).collect::<std::collections::BTreeMap<_,_>>())).collect::<std::collections::BTreeMap<_,_>>(),
        "diagnostics":diagnostics,"stats":{"events":events.len(),"tokens":tokens.len()}},"truncated":[]})
}
