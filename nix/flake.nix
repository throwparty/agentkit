{
  description = "throwparty/agentkit";
  inputs = {
    # acp-tck is a plain source tree, not a flake; uv2nix resolves its
    # committed uv.lock into a hermetic Python environment.
    acp-tck = {
      url = "github:agentclientprotocol/acp-tck";
      flake = false;
    };
    flake-utils.url = "github:numtide/flake-utils";
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs/release-26.05";
    pyproject-build-systems = {
      url = "github:pyproject-nix/build-system-pkgs";
      inputs.pyproject-nix.follows = "pyproject-nix";
      inputs.uv2nix.follows = "uv2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pyproject-nix = {
      url = "github:pyproject-nix/pyproject.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    throwparty = {
      url = "git+ssh://git@github.com/throwparty/nix";
      inputs = {
        flake-utils.follows = "flake-utils";
        nixpkgs.follows = "nixpkgs";
      };
    };
    uv2nix = {
      url = "github:pyproject-nix/uv2nix";
      inputs.pyproject-nix.follows = "pyproject-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      acp-tck,
      flake-utils,
      naersk,
      nixpkgs,
      pyproject-build-systems,
      pyproject-nix,
      rust-overlay,
      self,
      throwparty,
      uv2nix,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import rust-overlay)
            throwparty.overlays.default
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
        naersk' = pkgs.callPackage naersk {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };
        inherit (pkgs.lib) getExe getExe';
        mingwBinutils = pkgs.pkgsCross.mingwW64.buildPackages.binutils;
        acpTck =
          let
            workspace = uv2nix.lib.workspace.loadWorkspace {
              workspaceRoot = "${acp-tck}";
            };
            pythonSet =
              (pkgs.callPackage pyproject-nix.build.packages {
                python = pkgs.python314;
              }).overrideScope
                (
                  pkgs.lib.composeManyExtensions [
                    pyproject-build-systems.overlays.default
                    (workspace.mkPyprojectOverlay { sourcePreference = "wheel"; })
                  ]
                );
          in
          pythonSet.mkVirtualEnv "acp-tck-env" workspace.deps.default;
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
              dbus
              goreleaser
              mold
              nixfmt
              openssl
              otel-desktop-viewer
              pkg-config
              prettier
              python3
              rustup
              sccache
              shellcheck
              syft
              toml-sort
              yq-go
              zig
              ;
            # mdformat with the frontmatter plugin: without it mdformat
            # rewrites `---` YAML frontmatter into a heading, corrupting
            # definition files (personas, actors, prompts).
            mdformat = pkgs.mdformat.withPlugins (ps: [ ps.mdformat-frontmatter ]);
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
                printf "prettier %s\n" "$(${getExe prettier} --version)"
                ${getExe syft} --version
                printf "shellcheck %s\n" "$(${getExe shellcheck} --version | grep '^version:' | awk '{print $2}')"
                printf "toml-sort %s\n" "$(${getExe toml-sort} --version)"
                printf "yq %s\n" "$(${getExe yq-go} --version 2>/dev/null || true)"
                ${getExe zig} version
              '';
            };
            rustShell = pkgs.mkShell {
              nativeBuildInputs = [
                acpTck
                bats
                cargo-auditable
                cargo-binstall
                cargo-deny
                cargo-zigbuild
                cosign
                goreleaser
                mdformat
                mingwBinutils
                mold
                nixfmt
                otel-desktop-viewer
                pkg-config
                prettier
                python3
                rustToolchain
                rustup-wrapped
                sccache
                shellcheck
                syft
                toml-sort
                yq-go
                zig
              ];
              buildInputs = [
                dbus
                openssl
              ];
              shellHook =
                let
                  # Embed Nix library paths in RPATH of cargo-built binaries so
                  # they can find Nix-provided shared libs (e.g. libdbus-1.so.3
                  # for agentkit-credential-keychain) at runtime without setting
                  # LD_LIBRARY_PATH globally (which breaks system binaries like git).
                  rpathFlags = pkgs.lib.concatMapStringsSep " " (p: "-C link-arg=-Wl,-rpath,${p}") (
                    pkgs.lib.splitString ":" (
                      pkgs.lib.makeLibraryPath [
                        dbus
                        openssl
                      ]
                    )
                  );
                in
                ''
                  cat ${rustToolVersions}
                  export RUSTUP_HOME="$PWD/.rustup"
                  export CARGO_HOME="$PWD/.cargo"
                  export RUSTFLAGS="${rpathFlags}''${RUSTFLAGS:+ $RUSTFLAGS}"
                  mkdir -p "$RUSTUP_HOME" "$CARGO_HOME"
                  rustup toolchain link nix "$(dirname "$(readlink -f "$(type -P rustc)")")/.."
                  rustup default nix
                  export PATH="$CARGO_HOME/bin:$PATH"
                '';
            };
          in
          (mergeShells [
            commonTools
            githubActions
            nodejs_24
            rustShell
          ]);

        packages =
          let
            lib = nixpkgs.lib;
            mkAgentkitBin =
              bin:
              let
                qualifiedBin = "agentkit-${bin}";
              in
              naersk'.buildPackage {
                pname = qualifiedBin;
                version = "0.1.0";
                src = ../.;
                cargoBuildFlags = [
                  "--package"
                  qualifiedBin
                  "--bin"
                  qualifiedBin
                ];
                nativeBuildInputs = with pkgs; [
                  pkg-config
                  dbus
                  openssl
                ];
                buildInputs = with pkgs; [
                  dbus
                  openssl
                ];

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
            agentkit-switchboard = mkAgentkitBin "switchboard";
            agentkit-tackle = mkAgentkitBin "tackle";
          };
        checks = {
          inherit (self.packages.${system})
            agentkit-lens
            agentkit-litterbox
            agentkit-switchboard
            agentkit-tackle
            ;
        };
      }
    )
    // {
      overlays.default = final: prev: {
        inherit (self.packages.${final.system})
          agentkit-lens
          agentkit-litterbox
          agentkit-switchboard
          agentkit-tackle
          ;
      };
    };
}
