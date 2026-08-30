{
  description = "notist";

  # nixConfig = {
  #   extra-substituters = [
  #     "https://mirrors.ustc.edu.cn/nix-channels/store"
  #   ];
  #   trusted-substituters = [
  #     "https://mirrors.ustc.edu.cn/nix-channels/store"
  #   ];
  # };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        inherit (pkgs) lib;
        devCraneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            targets = [ "wasm32-unknown-unknown" ];
            extensions = [ "rust-src" ];
          }
        );

        # 打包用 stable 工具链即可（CI 也是 stable）
        craneLib = crane.mkLib pkgs;

        # build.rs 会把 docs/ 与 skills/notist 嵌入二进制，
        # 因此除了 Cargo 源码外还需要保留这两个目录。
        src = lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            (craneLib.filterCargoSources path type)
            || (lib.hasInfix "/docs/" path)
            || (lib.hasInfix "/skills/" path);
        };

        commonArgs = {
          inherit src;
          pname = "notist";
          strictDeps = true;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        notist = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoExtraArgs = "--locked --package notist-cli";
            # 测试需要两处可写目录，沙箱里 HOME（/homeless-shelter）不可写：
            # - XDG_CACHE_HOME：搜索索引缓存（见 notist-service 的 search_cache_path）
            # - NOTIST_DATA_DIR：内嵌官方文档的同步根（见 notist-cli 的 notist_data_root，
            #   每个命令入口都会 ensure_synced），这是 #1 里 checkPhase 真正的失败点，
            #   macOS 上表现为 Read-only file system，Linux 上是 Permission denied。
            # 不用字面量 /build：那是 Linux 沙箱的构建目录，macOS 上不存在；
            # $TMPDIR 在两个平台的沙箱里都指向可写的构建目录。
            preCheck = ''
              export XDG_CACHE_HOME="$TMPDIR/xdg-cache"
              export NOTIST_DATA_DIR="$TMPDIR/notist-data"
            '';
            meta = {
              description = "Notist CLI";
              mainProgram = "notist";
              license = with lib.licenses; [
                mit
                asl20
              ];
            };
          }
        );
      in
      {
        packages = {
          inherit notist;
          default = notist;
        };

        apps.default = {
          type = "app";
          program = lib.getExe notist;
        };

        checks = {
          inherit notist;
        };

        devShells.default = devCraneLib.devShell {
          packages =
            [ ]
            ++ (with pkgs; [
              git-cliff
              # cargo-release
              cargo-edit
              samply
              # cargo-udeps 依赖 nightly，stable 工具链下不可用
              # cargo-udeps
              miniserve
              # 与 plugins/mermaid-web 的 wasm-bindgen crate 版本严格一致，
              # 否则生成的胶水与运行时 ABI 不匹配。
              wasm-bindgen-cli
              # mdbook-katex
              # mdbook-i18n-helpers
            ]);
        };
      }
    );
}
