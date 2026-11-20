# Roadmap

Planned work, roughly in priority order within each section. Nothing here is
promised; open an issue if something matters to you.

## Distribution

- **Publish to crates.io**, so Rust users can `cargo install grepum`. Today
  grepum ships as prebuilt binaries on GitHub Releases (`install.sh`,
  `grepum upgrade`) or builds from a checkout with `./install.sh --from-source`.
  Publishing also reserves the name; `cargo publish --dry-run` already passes.
- Homebrew tap (`brew install grepum/tap/grepum`).

## Analysis

- Call-site inference for untyped parameters.
- C++ overload resolution by argument types (calls currently link to one
  overload of a set).
- Templates beyond the known Unreal Engine helpers.
- Element types for loop variables (`for b in &facts.bindings`), so members
  and calls on them link.
- C++ export macros other than Unreal's `*_API` (`class LEVELDB_EXPORT Status`
  is currently misparsed).
- Python decorators as call edges.
- Community detection (clusters of related symbols) to improve `grepum summary`.

## Storage and scale

- Pack L1 facts into a single file for very large repositories.
- Share L2 graphs through `refs/notes` or a ref, so CI can publish graphs that
  every clone reuses.

## Evaluation

- Hand-labelled call-edge accuracy on the real-repository benchmark set (today
  accuracy is only scored on the synthetic projects).
- Agent-outcome benchmark: the same code-navigation questions answered with
  grepum, with graphify and with plain grep, comparing correctness, tokens and
  time.
