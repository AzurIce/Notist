//! Captures one mermaid dispatch as fixture bytes for the JS conformance test.

fn main() -> Result<(), String> {
    let out_dir = std::env::args()
        .nth(1)
        .ok_or("usage: gen-mermaid <out-dir>")?;
    gen_fixtures::capture(
        &out_dir,
        "mermaid",
        notist_plugin_sdk::build_guest_state::<notist_plugin_mermaid_wasm::Plugin>(),
        gen_fixtures::mermaid_request()?,
    )
}
