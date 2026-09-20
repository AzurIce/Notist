use std::path::Path;
use tiny_http::{Header, Response, Server};
const PAGE: &str = r#"<!doctype html><html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Notist</title><style>body{font:16px system-ui;margin:32px auto;max-width:960px;padding:0 20px;line-height:1.6}nav{border-bottom:1px solid #ddd;padding:12px 0}main{overflow-wrap:anywhere}pre{overflow-x:auto;white-space:pre}code{white-space:pre-wrap}pre code{white-space:pre}[data-tight=true]>li>p{margin:0}notist-error,pre#error{color:#a21d32}canvas,svg{max-width:100%}</style><nav><strong>Notist</strong> <select aria-label="Module" id="modules"></select></nav><pre id="error"></pre><main></main><script type="module">
import {mount} from '/renderer.js';
let last='',busy=false;
const select=document.querySelector('select');
select.onchange=()=>{last='';refresh()};
async function refresh(){if(busy)return;busy=true;try{const response=await fetch('/content?module='+encodeURIComponent(select.value));const data=await response.json();if(!response.ok)throw Error(data.error);if(!select.options.length){for(const name of data.modules)select.add(new Option(name,name));select.value=data.entry;}const next=JSON.stringify(data);if(next!==last){await mount(document.querySelector('main'),data.content,data.components,location.origin+'/',data.attributes);last=next;}document.querySelector('#error').textContent=data.warnings.join('\n');}catch(e){document.querySelector('#error').textContent=e.message}finally{busy=false}}
refresh();setInterval(refresh,1000);
</script></html>"#;
pub fn serve(root: &Path, address: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    notist_analysis::package::load(&root)?;
    let server = Server::http(address).map_err(|e| e.to_string())?;
    eprintln!("Preview: http://{}", server.server_addr());
    for request in server.incoming_requests() {
        let url = request.url();
        let (status, mime, bytes) = if url == "/" {
            (200, "text/html", PAGE.as_bytes().to_vec())
        } else if url == "/renderer.js" {
            (
                200,
                "text/javascript",
                notist_html::RENDERER_JS.as_bytes().to_vec(),
            )
        } else {
            match notist_analysis::package::load(&root) {
                Err(error) => (
                    500,
                    "application/json",
                    serde_json::json!({"error":error}).to_string().into_bytes(),
                ),
                Ok(mut loaded) => {
                    if url.starts_with("/content?") {
                        let query = url.split_once('?').map_or("", |(_, v)| v);
                        let key = url::form_urlencoded::parse(query.as_bytes())
                            .find(|(name, _)| name == "module")
                            .map(|(_, value)| value.into_owned())
                            .unwrap_or_default();
                        let entry = if loaded.runtime.sources.contains_key(&key) {
                            key
                        } else {
                            loaded.entry.clone()
                        };
                        let result = loaded.runtime.evaluate(&entry);
                        (200,"application/json",serde_json::json!({"entry":entry,"modules":loaded.runtime.sources.keys().collect::<Vec<_>>(),"content":result.content.to_json(),"attributes":result.attributes.iter().map(|(k,v)|(k,v.to_json())).collect::<std::collections::BTreeMap<_,_>>(),"warnings":result.warnings,"components":loaded.components}).to_string().into_bytes())
                    } else if let Some(bytes) = loaded.resources.get(url.trim_start_matches('/')) {
                        (
                            200,
                            if url.ends_with(".js") {
                                "text/javascript"
                            } else if url.ends_with(".css") {
                                "text/css"
                            } else {
                                "application/octet-stream"
                            },
                            bytes.clone(),
                        )
                    } else {
                        (404, "text/plain", b"Not found".to_vec())
                    }
                }
            }
        };
        let response = Response::from_data(bytes)
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", mime).unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
        let _ = request.respond(response);
    }
    Ok(())
}
