{
  description = "throwparty/agentkit";
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:NixOS/nixpkgs/release-25.11";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    throwparty = {
      url = "git+ssh://git@github.com/throwparty/nix";
      inputs.flake-utils.follows = "flake-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      flake-utils,
      nixpkgs,
      rust-overlay,
      throwparty,
      self,
    }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import rust-overlay)
          ];
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [
            "wasm32-wasip1"
            "x86_64-apple-darwin"
            "x86_64-pc-windows-gnu"
            "aarch64-unknown-linux-gnu"
            "aarch64-apple-darwin"
          ];
        };
        inherit (pkgs.lib) getExe getExe';
        mingwBinutils = pkgs.pkgsCross.mingwW64.buildPackages.binutils;
      in
      {
        devShells.default =
          let
            inherit (throwparty.devShells.${system}) commonTools githubActions nodejs_24;
            inherit (throwparty.lib) mergeShells mkToolVersions;
            inherit (pkgs)
              bats
              cargo-auditable
              cargo-binstall
              cargo-deny
              cargo-zigbuild
              cosign
              goreleaser
              mdformat
              nixfmt
              openssl
              otel-desktop-viewer
              pkg-config
              python3
              rustup
              syft
              yq-go
              zig
              ;
            rustup-wrapped = pkgs.writeShellScriptBin "rustup" "
              case \"\${1}_\${2}_\" in
                target_add_|target_install_) exit 0 ;;
                *) exec ${pkgs.rustup}/bin/rustup \"\$@\" ;;
              esac
            ";
            rustToolVersions = mkToolVersions {
              inherit pkgs;
              name = "default";
              commands = ''
                ${getExe mdformat} --version
                ${getExe nixfmt} --version
                ${getExe openssl} version
                ${getExe otel-desktop-viewer} --version
                ${getExe python3} --version
                ${getExe' rustToolchain "cargo"} --version
                printf "goreleaser %s\n" "$(${getExe goreleaser} --version | grep GitVersion | awk '{print $2}')"
                printf "pkg-config %s\n" "$(${getExe pkg-config} --version | head -n 1)"
                ${getExe' rustToolchain "rustc"} --version
                ${getExe bats} --version
                printf "cargo-binstall %s\n" "$(${getExe cargo-binstall} --version 2>/dev/null || true)"
                printf "cargo-zigbuild %s\n" "$(${getExe cargo-zigbuild} --version 2>/dev/null || true)"
                ${getExe cosign} --version
                ${getExe syft} --version
                printf "yq %s\n" "$(${getExe yq-go} --version 2>/dev/null || true)"
                ${getExe zig} version
              '';
            };
            rustShell = pkgs.mkShell {
              nativeBuildInputs = [
                bats
                cargo-auditable
                cargo-binstall
                cargo-deny
                cargo-zigbuild
                cosign
                goreleaser
                mdformat
                mingwBinutils
                nixfmt
                openssl
                otel-desktop-viewer
                pkg-config
                python3
                rustToolchain
                rustup-wrapped
                syft
                yq-go
                zig
              ];
              shellHook = ''
                cat ${rustToolVersions}
                export RUSTUP_HOME="$PWD/.rustup"
                export CARGO_HOME="$PWD/.cargo"
                mkdir -p "$RUSTUP_HOME" "$CARGO_HOME"
                rustup toolchain link nix "$(dirname "$(readlink -f "$(type -P rustc)")")/.."
                rustup default nix
                export PATH="$CARGO_HOME/bin:$PATH"
              '';
            };
          in (mergeShells [ commonTools githubActions nodejs_24 rustShell ]);

        packages =
            let
              lib = nixpkgs.lib;
              mkAgentkitBin =
                bin:
                let
                  qualifiedBin = "agentkit-${bin}";
                  commonCargoFlags = [
                    "--package" qualifiedBin
                    "--bin" qualifiedBin
                  ];
                in
                pkgs.rustPlatform.buildRustPackage {
                  pname = qualifiedBin;
                  version = "0.1.0";

                  src = ../.;
                  cargoBuildFlags = commonCargoFlags;
                  cargoTestFlags = commonCargoFlags;
                  cargoDepsName = "agentkit";
                  cargoHash = "sha256-2fju6VTEcnwvnd06rC7Xw+UAeBJ3RKpuYqY5WwqFpQI=";

                  meta = {
                    description = "Provides fetch and search tools backed by various search engines.";
                    homepage = "https://agentkit.throw.party/docs/user/${bin}/";
                    license = lib.licenses.asl20;
                    mainProgram = "agentkit-${bin}";
                  };
                };
            in
            {
              agentkit-lens = mkAgentkitBin "lens";
              agentkit-litterbox = mkAgentkitBin "litterbox";
            };
        checks = {
          inherit (self.packages.${system})
            agentkit-lens
            agentkit-litterbox
            ;
        };
      }
    )
    // {
      overlays.default =
        final: prev:
        {
          inherit (self.packages.${final.system})
            agentkit-lens
            agentkit-litterbox
            ;
        };
      };
}
