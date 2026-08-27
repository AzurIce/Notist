//! Captures one shader dispatch as fixture bytes for the JS conformance test.

fn main() -> Result<(), String> {
    let out_dir = std::env::args()
        .nth(1)
        .ok_or("usage: gen-shader <out-dir>")?;
    gen_fixtures::capture(
        &out_dir,
        "shader",
        notist_plugin_sdk::build_guest_state::<notist_plugin_shader_wasm::Plugin>(),
        gen_fixtures::shader_request(),
    )
}
