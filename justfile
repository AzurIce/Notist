set windows-shell := ["pwsh", "-Command"]

[working-directory: 'docs']
preview:
    cargo run -- preview

stop:
    cargo run -p notist-cli -- daemon stop

# 把 plugins/ 下所有 semantic crate 编成 core module 并刷新包内产物
build-plugins:
    nix develop ~/Files/notist -c cargo build --profile plugin-release --target wasm32-unknown-unknown -p notist-plugin-component-echo -p notist-plugin-shader-wasm -p notist-plugin-mermaid-wasm
    cp target/wasm32-unknown-unknown/plugin-release/notist_plugin_component_echo.wasm plugins/component-echo/semantic.wasm
    cp target/wasm32-unknown-unknown/plugin-release/notist_plugin_shader_wasm.wasm plugins/shader/semantic.wasm
    cp target/wasm32-unknown-unknown/plugin-release/notist_plugin_mermaid_wasm.wasm plugins/mermaid/semantic.wasm
