#!/usr/bin/env python3
"""Serve the .notc web debugger: app at /app/, wasm-bindgen glue at /pkg/."""
import http.server
import os
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))


class Handler(http.server.SimpleHTTPRequestHandler):
    # 显式 MIME：跨平台 mimetypes 注册表对 .wasm/.js 不稳定（Windows 常缺 .wasm）。
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".wasm": "application/wasm",
    }

    def translate_path(self, path):
        # wasm-bindgen 的产物是 notist_next_web_bg.wasm；为方便外部按 crate 名
        # 引用（/pkg/notist_next_web.wasm），在同目录里别名到实际文件。
        path = super().translate_path(path)
        if path.endswith("/pkg/notist_next_web.wasm"):
            return path[: -len("notist_next_web.wasm")] + "notist_next_web_bg.wasm"
        return path


def main() -> None:
    os.chdir(ROOT)
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
    with http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler) as server:
        print(f"serving {ROOT} at http://127.0.0.1:{port}/app/")
        server.serve_forever()


if __name__ == "__main__":
    main()
