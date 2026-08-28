set windows-shell := ["pwsh", "-Command"]

[working-directory: 'docs']
preview:
    cargo run -- preview

stop:
    cargo run -p notist-cli -- daemon stop

# 把 plugins/ 下所有 semantic crate 编成 core module 并刷新包内产物
build-plugins:
    cd plugins && nix develop ~/Files/notist -c cargo build --target wasm32-unknown-unknown --release
    cp plugins/component-echo/semantic/target/wasm32-unknown-unknown/release/notist_plugin_component_echo.wasm plugins/component-echo/semantic.wasm
    cp plugins/shader/semantic/target/wasm32-unknown-unknown/release/notist_plugin_shader_wasm.wasm plugins/shader/semantic.wasm
    cp plugins/mermaid/semantic/target/wasm32-unknown-unknown/release/notist_plugin_mermaid_wasm.wasm plugins/mermaid/semantic.wasm
