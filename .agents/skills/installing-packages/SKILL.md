---
name: installing-packages
description: Installing tools and packages via Nix, npm, cargo, pip, and other package managers
---

## When to use me

When you need to install a binary tool or library and need to choose which package manager to use.

## Decision hierarchy

1. **Nixpkgs** — preferred for all CLI tools and libraries. Add to `nativeBuildInputs` in the nix flake devshell and to `rustToolVersions` for version-printing. Nixpkgs names sometimes differ from upstream (e.g. `bats` not `bats`). Search with `nix search nixpkgs <name>` to confirm the attribute path.

2. **Cargo** — for Rust crates not in nixpkgs. Use `cargo install` or `cargo binstall` for pre-built binaries.

3. **npm** — for Node.js tools. Use `npm install -g` or `npm install --save-dev` for project-local tools.

4. **pip** — for Python packages (avoid when a nixpkgs equivalent exists). Use `pip install` or `pipx` for isolation.

5. **Direct download** — for statically linked binaries from GitHub releases. Prefer pinning by commit SHA over version tags.

## Adding to the nix devshell

The devshell is defined in `nix/flake.nix`. Add a package to:

1. The `inherit (pkgs)` block at the top of `devShells.default`
2. The `nativeBuildInputs` list in `rustShell`
3. The `commands` string in `rustToolVersions` for version-printing at shell entry

Example adding `bats`:

```nix
# inherit
inherit (pkgs)
  bats           # <-- added
  cargo-auditable
  ...

# nativeBuildInputs
rustShell = pkgs.mkShell {
  nativeBuildInputs = [
    bats           # <-- added
    cargo-auditable
    ...
  ];
};

# version check
commands = ''
  ...
  ${getExe bats} --version    # <-- added
  ...
'';
```

After editing `nix/flake.nix`, run `direnv reload` to rebuild the shell.

## Checking what's available

```bash
# Search nixpkgs
nix search nixpkgs <name>

# List installed nix packages
nix profile list

# Check if a package exists in the devshell
direnv exec $PWD which <tool>
direnv exec $PWD <tool> --version
```
