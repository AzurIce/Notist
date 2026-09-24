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

        # Tests and include_str! need package fixtures and the browser renderer.
        src = lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            (craneLib.filterCargoSources path type)
            || (lib.hasPrefix (toString ./. + "/examples/") path)
            || (lib.hasInfix "/crates/notist-html/" path);
        };

        commonArgs = {
          inherit src;
          pname = "notist";
          strictDeps = true;
          cargoExtraArgs = "--locked --package notist-cli -j8";
          cargoTestExtraArgs = "-- --test-threads=4";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        notist = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            meta = {
              description = "Notist language tools";
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
              bun
              # 与 editor/crates/editor-wasm 的 wasm-bindgen crate 版本严格一致，
              # 否则生成的胶水与运行时 ABI 不匹配。
              wasm-bindgen-cli
              # mdbook-katex
              # mdbook-i18n-helpers
            ]);
        };
      }
    );
}
