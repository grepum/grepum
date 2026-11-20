---
name: grepum
description: Navigate code through the grepum call graph instead of grepping. Use for "where is X defined", "who calls X", "what does X call", "what breaks if I change X", "what did this branch change structurally", and before editing a symbol you haven't read yet. Works for Python, TypeScript/JavaScript, Rust, Go, C/C++ and Unreal Engine projects, in any worktree or at any commit.
---

# grepum

`grepum` answers structural questions about the code in milliseconds, from
a graph cached inside `.git`. It always reflects the **current worktree,
including uncommitted edits**. There is no index step to remember.

## When to reach for it

| Question | Command |
|---|---|
| Where is something? | `grepum find <words>`: ranked; `order service`, `apply damage` |
| Who calls it? | `grepum callers <Symbol>` (`--depth 3` for transitive impact) |
| What does it call? | `grepum callees <Symbol>` |
| What is in this file / class / directory? | `grepum outline <path\|dir\|Type>`: one line per definition with its signature, line range and caller count, no bodies; `-d 1` for top level only. Pick one, then `grepum show path:line` |
| Read its code | `grepum show <Symbol>`: exactly the function's body plus its doc comment and decorators; `--callees` adds what it calls; `grepum callers X \| grepum show -` reads every caller |
| Orient on a feature | `grepum context <words>`: matches plus 1-hop callers and callees |
| What did this branch change? | `grepum diff main`: symbols and call edges added or removed |
| Same question on another branch/commit | add `--rev <rev>` to any command; no checkout needed |
| When/why did this function change? Where did it come from? | `grepum log Class.method`: commits that added, modified, moved (across files) or removed it |
| There used to be a function for X; where did it go? | `grepum find <words> --history`: also lists deleted symbols with the commit that removed them and a `git show <commit>^:<path>` to read the last version |

Symbols can be written `Name`, `Class.method`, `Class::Method`, or
`path/suffix.py:Class.method` to disambiguate. Add `--json` for structured
output.

## Workflow

1. Start with `find` or `context` to locate the code. Don't grep for
   identifiers the graph can resolve.
2. Before opening a file to find your way around it, `grepum outline <file>`
   (or a directory, or a type to list its members wherever they're written).
   Read the code with `grepum show <Symbol>` (or pipe `find`/`callers`
   output into `grepum show -`) instead of opening whole files. Long
   functions are cut to their start and end; `--max-lines 0` shows all.
   `rg -n <pattern> . | grepum show -` shows the functions around text matches.
3. Before changing a function's signature or behaviour, run
   `grepum callers <Symbol> --depth 2` and check every caller you'll affect.
4. After your edits, `grepum diff HEAD` shows what you changed structurally:
   new/removed functions and call edges. Use it to sanity-check a refactor.

## Reading the output

- `<-` caller, `->` callee, indented by depth.
- `(inferred)` means the target was chosen because the name is unique, not
  proven by imports/types. Verify it by reading the call site when it matters.
- Calls whose target can't be proven are **omitted**, not guessed. If a caller
  you expect is missing, the call goes through something the graph can't see:
  - an untyped parameter
  - dynamic dispatch through a base-class pointer
  - reflection (`getattr`, `BindUFunction("Name")`)
  - code outside the repo

  Fall back to `grep -n "<name>("` for that one case.

## Unreal Engine specifics

- `UCLASS`/`UFUNCTION`/`UPROPERTY` specifiers appear as tags, e.g.
  `[UFUNCTION, Server, Reliable]`.
- Calls to `Server`/`Client`/`NetMulticast` RPCs and `BlueprintNativeEvent`s
  link to the `<Name>_Implementation` body, which is where the code is.
  `callers AMyActor::ServerFire` includes both.
- `BindAction(..., &AMyChar::Fire)`, `AddDynamic(this, &AMyChar::OnHit)`,
  and `SetTimer(..., &AMyChar::Tick2, ...)` count as calls from the function
  that binds them. This is how you find what triggers an input or delegate handler.
- `BlueprintImplementableEvent`s have no C++ body; the graph points at the
  header declaration. Blueprint-side callers aren't visible. They live in
  `.uasset` files.
- `Cast<T>`, `NewObject<T>`, `CreateDefaultSubobject<T>`, `SpawnActor<T>` and
  `GetSubsystem<T>` type their results, so `Cast<AFoo>(x)->Bar()` resolves.

## Worktrees

All worktrees of a repository share one cache in the git common directory.
A fresh `git worktree add` (or a Claude Code worktree session) is queryable
immediately. Each worktree's uncommitted edits stay private to it.

## What grepum can't see

grepum is a static call graph. Use it to find what calls a symbol before you
change it, then read the code; don't treat it as the whole truth.

- **"(no callers found)" is not proof of dead code.** Calls through receivers
  it can't type (untyped parameters, macros, reflection, delegates bound by
  name, data-driven dispatch) can be missed. Confirm with `rg -w <name>`
  before deleting anything.
- **Iteration by type is not a call.** Code that walks every
  `ACharacter` with `TActorIterator` never "calls" the horse, so no query links
  the two. Search for the type name instead.
- **Data flow is not a call.** Two systems sharing a map key or a global are
  invisible to it.
- **Config and assets aren't in the graph** (`.ini`, `.json`, `.uasset`, data
  tables). Grep those directly.
- **`find` matches names, not behaviour.** If the first results are one class,
  try the words a neighbouring class would use, or `grepum context <words>`.

