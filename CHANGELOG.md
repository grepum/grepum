# Changelog

## 0.4.0

- **`grepum show <symbol>`** prints a definition's source, exactly as far as
  its syntax reaches (braces, indentation), with what belongs to it: doc
  comments, attributes, decorators, `template<...>`, Unreal `UFUNCTION(...)`.
  A C++ prototype shows its out-of-line body. `show -` reads targets from
  stdin: any `path:line` (grepum's own output, `rg -n`, compiler errors)
  names the function around that line, so `grepum callers X | grepum show -`
  prints every caller. `--callees` adds what it calls, `--rev` reads a commit,
  `--json` for tools. Definitions over `--max-lines` (200) keep their
  signature, start and end.
- **Each language is a module behind one interface** (`src/lang/`): its
  syntax handling, its module resolution (imports, includes, packages) and
  its resolution rules as data. The extractor and resolver no longer branch
  on the language. Output is unchanged on the benchmark repos, except:
- **Go has its own list of standard-library method names**, so calls like
  `cmd.Wait()` or `ticker.Stop()` on an unknown receiver are no longer
  guessed to be a project method (11 wrong links fewer on fzf).
- Output order is deterministic (inherit edges and imports were in hash order).

## 0.3.0

TypeScript, scored against the TypeScript compiler on two real repos
(`benchmark/ts/run.sh`): hono 88.9% -> 91.9% of calls found and 95.9% ->
98.7% of links right; excalidraw 79.4% -> 97.1% found, 95.9% -> 98.2% right.

- **tsconfig `paths` and `baseUrl`**, including ones inherited through
  `extends`, and **workspace packages** (`import { x } from '@acme/geo'`)
  resolve to the source files. Edits to tsconfig/package.json in a dirty
  worktree take effect immediately.
- **JSX components are calls**: `<Widget />` and `<Toolbar.Item />` link to
  the component, including ones wrapped in `memo`/`forwardRef`.
- **Exports without `from`**: `export default Scene;`, `export { a as b }`
  and `export { importedName }` are followed.
- **Getters and interface properties type members**: `get req(): HonoRequest`
  makes `c.req.header()` resolve.
- **Object-literal namespaces**: `const util = { fmt() {}, parse: (s) => s }`
  makes `util.fmt()` link to `util.fmt` (shown as kind `object`).
- **Generic constraints**: `<T extends Repo>(r: T) => r.find()`.
- **Fewer wrong links**: `console.*` and other JS globals are never project
  methods; parameters and locals (also destructured ones) shadow same-named
  functions elsewhere, from the line they're bound on, in every language;
  functions nested in another function are only reachable from inside it; an
  interface or type alias is never a call target.
- **5-day dependency cooldown.** `cargo update` (Cargo 1.100+) skips crate
  versions younger than 5 days, and the release audit fails if `Cargo.lock`
  holds one. clap and regex were pinned back to their previous releases
  (4.6.7, 1.13.1); 0.2.1 had shipped versions 2-3 days old.

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
