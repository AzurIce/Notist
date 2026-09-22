set windows-shell := ["pwsh", "-Command"]

test:
    cargo test -j8 --workspace --locked -- --test-threads=4

test-plugin-sdk:
    cargo build -p notist-plugin-sdk --example semantic --release --target wasm32-unknown-unknown --target-dir target
    cargo test -p notist-analysis --test plugin_sdk -- --ignored

demo:
    cargo run -j8 -p notist-cli -- eval examples/demo --html

check package="examples/workspace":
    cargo run -j8 -p notist-cli -- check {{package}}

preview package="examples/workspace":
    cargo run -j8 -p notist-cli -- preview {{package}}

lsp:
    cargo run -j8 -p notist-cli -- lsp

# 编译编辑器绑定到 wasm32 并生成 wasm-bindgen web glue（editor/pkg/）
web-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    test "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.121" || { echo "需要 wasm-bindgen-cli 0.2.121（当前：$(wasm-bindgen --version)）"; exit 1; }
    cargo build -j4 --release --target wasm32-unknown-unknown --manifest-path editor/Cargo.toml --target-dir target
    wasm-bindgen --target web --out-dir editor/pkg target/wasm32-unknown-unknown/release/notist_editor.wasm

# 同步 examples fixtures 到 editor/fixtures/（幂等）
web-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    mkdir -p editor/fixtures
    cargo run -j4 -p notist-analysis --example build_fixture

# 本地静态服务：http://127.0.0.1:8000/app/
web-serve port="8000":
    miniserve --interfaces 127.0.0.1 --port {{port}} --index index.html "{{justfile_directory()}}/editor"

# 独立文档内核，分别生成浏览器和 Node 接口；不包含语言服务。
editor-core-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    test "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.121"
    cargo build -j4 --release --target wasm32-unknown-unknown --manifest-path editor/Cargo.toml --target-dir target -p notist-editor-core-wasm
    wasm-bindgen --target web --out-dir editor/pkg-core target/wasm32-unknown-unknown/release/notist_editor_core_wasm.wasm
    wasm-bindgen --target nodejs --out-dir editor/scripts/pkg-core-node target/wasm32-unknown-unknown/release/notist_editor_core_wasm.wasm

# Rust 契约检查及同一内核的 native/Wasm 对拍。
editor-core-test:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    cargo test -j4 --manifest-path editor/Cargo.toml -p notist-editor-core
    cargo build -j4 --manifest-path editor/Cargo.toml -p notist-editor-core --example replay
    node --test editor/core/contracts.test.mjs

# wasm 与 native 快照对拍（需要 node；platform 归一化后逐字节比较）
web-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    if ! command -v node >/dev/null 2>&1; then echo "node 不可用，跳过 web-compare"; exit 0; fi
    wasm-bindgen --target nodejs --out-dir editor/scripts/pkg-node target/wasm32-unknown-unknown/release/notist_editor.wasm
    node editor/scripts/compare.mjs
