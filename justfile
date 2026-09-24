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

# 独立文档内核，分别生成浏览器和 Bun 接口；不包含语言服务。
editor-document-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    test "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.121"
    cargo build -j4 --release --target wasm32-unknown-unknown --manifest-path editor/Cargo.toml --target-dir target -p notist-editor-node-wasm
    wasm-bindgen --target web --out-dir editor/pkg-editor target/wasm32-unknown-unknown/release/notist_editor_node_wasm.wasm
    wasm-bindgen --target nodejs --out-dir editor/scripts/pkg-editor-node target/wasm32-unknown-unknown/release/notist_editor_node_wasm.wasm

# Rust 契约检查及同一内核的 native/Wasm 对拍。
editor-document-test:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    cargo test -j4 --manifest-path editor/Cargo.toml -p notist-editor-document
    cargo build -j4 --manifest-path editor/Cargo.toml -p notist-editor-document --example replay
    bun test ./editor/document/contracts.test.mjs

# wasm 与 native 快照对拍（需要 Bun；platform 归一化后逐字节比较）
web-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    if ! command -v bun >/dev/null 2>&1; then echo "Bun 不可用，跳过 web-compare"; exit 0; fi
    wasm-bindgen --target nodejs --out-dir editor/scripts/pkg-node target/wasm32-unknown-unknown/release/notist_editor.wasm
    bun editor/scripts/compare.mjs

# 构建浏览器 document/node 绑定和单进程 native 节点。
editor-node-build: editor-document-build
    cargo build -j4 --manifest-path editor/Cargo.toml -p notist-editor-node --features native

# 同步状态机、native 运行时、Wasm 契约及真实 Chromium/TURN 集成。
editor-node-test: editor-node-build
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    cargo test -j4 --manifest-path editor/Cargo.toml -p notist-editor-node --features native
    if [ ! -f editor/pkg/notist_editor_bg.wasm ]; then just web-build; fi
    bun install --cwd editor/experiments/source-projection --frozen-lockfile
    bun run --cwd editor/experiments/source-projection build
    bun install --cwd editor/node --frozen-lockfile
    bun run --cwd editor/node test

# 使用同一 library runtime 运行 headless 节点。
editor-node-run config="editor/node/examples/local.toml":
    cargo run --manifest-path editor/Cargo.toml -p notist-editor-node --features native -- --config {{config}}
