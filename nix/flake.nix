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
          targets = [ "wasm32-wasip1" ];
        };
        inherit (pkgs.lib) getExe getExe';
      in
      {
        devShells.default =
          let
            inherit (throwparty.devShells.${system}) commonTools githubActions nodejs_24;
            inherit (throwparty.lib) mergeShells mkToolVersions;
            inherit (pkgs)
              cargo-auditable
              cargo-deny
              cosign
              goreleaser
              mdformat
              nixfmt
              openssl
              otel-desktop-viewer
              pkg-config
              python3
              syft
              ;
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
                ${getExe cosign} --version
                ${getExe syft} --version
              '';
            };
            rustShell = pkgs.mkShell {
              nativeBuildInputs = [
                cargo-auditable
                cargo-deny
                cosign
                goreleaser
                mdformat
                nixfmt
                openssl
                otel-desktop-viewer
                pkg-config
                python3
                rustToolchain
                syft
              ];
              shellHook = "\ncat ${rustToolVersions}";
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
                  cargoHash = "sha256-OeGe8qQS9XlBNeoHVCRE+aywxsHslqbCxtm6j9RtnWw=";

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
