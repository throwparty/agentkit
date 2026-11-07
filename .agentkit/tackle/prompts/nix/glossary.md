<!-- Resource of the `nix` prompt: read this for the words, the prompt for the behaviour. -->

# Nix vocabulary

Terms from Nix, as Litterbox uses them. Nix is upstream: this file does not
define Nix, it records the terms a component here needs, pins them to what the
installed version does, and names any place we narrow or extend one.

Verified against `nix (Nix) 2.34.9`. Commands that establish the behavioural
notes live in the `nix` capability prompt (.agentkit/tackle/prompts/nix/PROMPT.md).

## Store and build

- **Derivation** — a build recipe written to the store as a `.drv` file: inputs,
  builder, arguments and output paths. Building one produces its outputs.
  ([manual](https://nix.dev/manual/nix/2.34/language/derivations))
- **Store path** — an entry under `/nix/store/<hash>-<name>`. Immutable once
  created; other store paths and files in the tree refer to it by full path, not
  by a search path. ([manual](https://nix.dev/manual/nix/2.34/store/store-path))
- **Realise** — make a derivation's outputs valid in the store, by building them
  or substituting them from a binary cache. A path that is not valid is not
  there; realisation is what makes it valid, and querying is not realisation.
- **Closure** — every store path reachable from a path by its recorded
  references, transitively. `nix path-info -r` prints it.
  ([glossary](https://nix.dev/manual/nix/2.34/glossary))
- **Nix build sandbox** — the isolation `nix-daemon` applies to a builder on
  Linux: separate namespaces, no network by default, no access outside the
  declared inputs. ([manual](https://nix.dev/manual/nix/2.34/command-ref/conf-file))
  Not to be confused with a **Litterbox sandbox**, which is a different
  mechanism with a different owner. Always qualify the word.

## Flakes and devShells

- **Flake** — a source tree containing `flake.nix`, declaring inputs and
  outputs, with `flake.lock` pinning the inputs. Evaluation of a flake is pure
  by default. ([concept](https://nix.dev/concepts/flakes))
- **Installable** — a flake reference plus an attribute path, such as `./nix` or
  `.#devShells.x86_64-linux.default`. Nix commands take installables wherever
  they take a package or a shell.
- **devShell** — a derivation (`mkShell` and equivalents) whose output carries a
  shell environment. `nix develop` enters it.
  ([manual](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-develop))
- **Shell hook** — the bash code a devShell carries, run by `nix develop` on
  entry. It both adds variables and overrides ones the stdenv baseline already
  sets, and it inherits whatever the calling shell exported.
- **`print-dev-env`** — `nix print-dev-env`, an experimental command that prints
  a devShell's environment as shell code, or as JSON with
  `{bashFunctions, variables}`. Each variable is tagged `exported`, `var` or
  `array`; an `exported` entry may legitimately be empty.
  ([manual](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-print-dev-env))

## Evaluation

- **Pure evaluation** — evaluation with no access to the ambient environment,
  `$HOME`, or paths not declared as inputs. Flake evaluation is pure. The shell
  hook is not evaluation: `nix develop` runs it as bash with the caller's
  exported environment, so its result depends on who ran it. The
  `print-dev-env --json` capture reports the baseline and does not run the hook
  itself; the shell-code output runs it when sourced.
- **Import-from-derivation (IFD)** — Nix code that builds a derivation during
  evaluation and imports its output. It is the reason evaluation can build.
  ([manual](https://nix.dev/manual/nix/2.34/language/import-from-derivation))

## Boundary

This file defines upstream terms only. A term Litterbox coins — **store view**,
**differential environment**, **read-only grant** — belongs to the component
that owns it, in `crates/agentkit-litterbox/CONTEXT.md`. If a term here is
narrowed for one component, the narrowing lives beside the term that component
uses, not in this file, because two components may narrow the same Nix word
differently.
