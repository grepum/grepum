# Changelog

## 0.2.1

- **C++: calls through members used without `this->` now resolve** (the
  Unreal default), including `TUniquePtr`/`TSharedPtr` chains such as
  `State->Rides->show(...)` and members inherited from base classes. On a
  1,100-file Unreal project this added 764 call edges; the 39 edges it
  removed were all wrong targets now replaced by the right class.
- **C++ type aliases are followed** (`using Core = parts::JobCore;`, typedefs),
  so receivers typed through an alias reach the real class.
- **`callers` is honest about misses**: "(no callers found)" plus a reminder to
  confirm with a text search before treating code as dead.
- **Unknown symbols suggest the closest names** instead of only pointing at
  `find`.
- **`find` keeps results broad**: at most three members of one class before
  other matches, so a neighbouring class isn't pushed off the list.
- The Claude Code skill and agent now spell out what a call graph can't see
  (iteration by type, shared data, config and asset files).

## 0.2.0

- **`grepum html`**: a self-contained, offline HTML viewer for the graph. A
  folder map with call traffic, and a focus view with a symbol's callers and
  callees two steps each way, plus search and shareable `#focus=` links.
- **Dependency audit before every release**: `cargo deny check` (RustSec
  advisories, licenses, bans, sources) gates the release workflow, and also
  runs on dependency changes and weekly.
- **Git hooks are safe around other people's hooks**: hooks in other
  languages are skipped instead of broken; a hooks directory that is part of
  the working tree (`core.hooksPath` to committed team hooks) is refused unless
  `--force`; existing hooks keep their permissions and are restored exactly on
  uninstall; the background refresh no longer inherits `GIT_INDEX_FILE` and
  friends from the hook.
- **Windows**: the Claude Code session hook works under Git Bash (no more
  `\\?\` paths), project-scope installs write a portable PATH-based hook
  without machine paths, and `grepum status` uses consistent separators.

## 0.1.0

First public release: content-addressed code graph in `.git`, shared across
worktrees; Python, TypeScript/JavaScript, Rust, Go and C/C++ (Unreal Engine 5);
`grepum log` and `find --history`; Claude Code skill, agent and session hook;
prebuilt binaries with `install.sh` and `grepum upgrade`.
