set windows-shell := ["pwsh", "-Command"]

test:
    cargo test -j8 --workspace --locked -- --test-threads=4

demo:
    cargo run -j8 -p notist-cli -- eval crates/notist-next/examples/demo --html

check package="examples/workspace":
    cargo run -j8 -p notist-cli -- check {{package}}

preview package="examples/workspace":
    cargo run -j8 -p notist-cli -- preview {{package}}

lsp:
    cargo run -j8 -p notist-cli -- lsp

# 编译语言核到 wasm32 并生成 wasm-bindgen web glue（web/pkg/）
web-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    test "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.121" || { echo "需要 wasm-bindgen-cli 0.2.121（当前：$(wasm-bindgen --version)）"; exit 1; }
    cargo build -j4 --release --target wasm32-unknown-unknown --manifest-path crates/notist-next/web/Cargo.toml --target-dir target
    wasm-bindgen --target web --out-dir crates/notist-next/web/pkg target/wasm32-unknown-unknown/release/notist_next_web.wasm

# 同步 examples fixtures 到 web/fixtures/（幂等）
web-fixtures:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    mkdir -p crates/notist-next/web/fixtures
    cargo run -j4 -p notist-next --example build_fixture

# 本地静态服务：http://127.0.0.1:8000/app/
web-serve port="8000":
    miniserve --interfaces 127.0.0.1 --port {{port}} --index index.html "{{justfile_directory()}}/crates/notist-next/web"

# wasm 与 native 快照对拍（需要 node；platform 归一化后逐字节比较）
web-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}"
    if ! command -v node >/dev/null 2>&1; then echo "node 不可用，跳过 web-compare"; exit 0; fi
    wasm-bindgen --target nodejs --out-dir crates/notist-next/web/scripts/pkg-node target/wasm32-unknown-unknown/release/notist_next_web.wasm
    node crates/notist-next/web/scripts/compare.mjs
