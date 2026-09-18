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

# —— notist-next web 调试器 ——

# 编译语言核到 wasm32 并生成 wasm-bindgen web glue（web/pkg/）
web-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    test "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.121" || { echo "需要 wasm-bindgen-cli 0.2.121（当前：$(wasm-bindgen --version)）"; exit 1; }
    cargo build -j4 --release --target wasm32-unknown-unknown --manifest-path crates-next/notist-next/web/Cargo.toml --target-dir target
    wasm-bindgen --target web --out-dir crates-next/notist-next/web/pkg target/wasm32-unknown-unknown/release/notist_next_web.wasm

# 同步 examples fixtures 到 web/fixtures/（幂等）
web-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    mkdir -p crates-next/notist-next/web/fixtures
    cargo run -j4 -p notist-next --example build_fixture

# 本地静态服务：http://127.0.0.1:8000/app/
web-serve:
    cd "{{justfile_directory()}}/crates-next/notist-next/web" && python3 serve.py

# wasm 与 native 快照对拍（需要 node；platform 归一化后逐字节比较）
web-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    if ! command -v node >/dev/null 2>&1; then echo "node 不可用，跳过 web-compare"; exit 0; fi
    wasm-bindgen --target nodejs --out-dir crates-next/notist-next/web/scripts/pkg-node target/wasm32-unknown-unknown/release/notist_next_web.wasm
    node crates-next/notist-next/web/scripts/compare.mjs
