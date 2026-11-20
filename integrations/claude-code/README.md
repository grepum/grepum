# grepum for Claude Code

A Claude Code plugin with three parts:

| Part | File | What it does |
|---|---|---|
| Skill | `skills/grepum/SKILL.md` | Teaches Claude when to use `grepum` instead of grep, and how to read its output (incl. Unreal specifics). Loaded on demand. |
| Subagent | `agents/code-navigator.md` | A read-only explorer (Haiku, so it's fast and cheap) that answers where/who-calls/impact questions with file:line evidence, keeping file dumps out of the main context. |
| Hook | `hooks/hooks.json` | On session start/resume/compact, injects `grepum summary` (size, languages, most-called symbols) so Claude knows the graph exists. |

No "remember to re-index after editing" rule is needed. Every query reflects
the worktree's uncommitted state, and new worktrees reuse the shared cache.

## Install

```
grepum install claude                   # ~/.claude: every project
grepum install claude --scope project   # ./.claude: this repo; commit it to share with the team
grepum install claude --dry-run         # show what would change
grepum uninstall claude [--scope ...]   # remove it again
```

Or let `./install.sh --claude user|project` do it after building the
binary. The installer:
- writes `skills/grepum/SKILL.md` and `agents/code-navigator.md` and marks
  them as its own. Files it didn't write are never overwritten unless you pass
  `--force`.
- merges one `SessionStart` entry into `settings.json`, keeping your other
  settings and hooks in order. In user scope the previous file is saved as
  `settings.json.grepum.bak`.
- points the hook at the installed binary's absolute path, so it doesn't
  depend on the PATH Claude Code's shell gets.

To try the plugin without installing anything:
`claude --plugin-dir /path/to/grepum/integrations/claude-code`
(requires `grepum` on your PATH).

## Optional always-on rule

To make Claude prefer the graph even before the skill triggers, add this to
the project's `CLAUDE.md`:

```markdown
## Code navigation
This repo has a grepum code graph. For "where is / who calls / what calls /
what breaks if" questions, use `grepum context|find|callers|callees` before
grep, and read only the line ranges it returns. Delegate multi-file
exploration to the `code-navigator` subagent.
```
