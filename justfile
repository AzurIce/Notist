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

# 校验提交的 semantic.wasm 与源码重建一致（合并/发布前跑）
check-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    cd plugins
    nix develop ~/Files/notist -c cargo build --profile plugin-release --target wasm32-unknown-unknown -p notist-plugin-component-echo -p notist-plugin-shader-wasm -p notist-plugin-mermaid-wasm
    cd ..
    for spec in component-echo:notist_plugin_component_echo shader:notist_plugin_shader_wasm mermaid:notist_plugin_mermaid_wasm; do
        dir="${spec%%:*}"; base="${spec##*:}"
        cmp "target/wasm32-unknown-unknown/plugin-release/${base}.wasm" "plugins/${dir}/semantic.wasm"             || { echo "plugins/${dir}/semantic.wasm 与源码不一致：先跑 just build-plugins 再提交"; exit 1; }
        echo "plugins/${dir}/semantic.wasm ✓"
    done
