#!/bin/sh
# grepum installer.
#
#   curl -fsSL https://raw.githubusercontent.com/grepum/grepum/main/install.sh | sh
#   ./install.sh                         install the latest release, then ask about Claude Code
#   ./install.sh --claude user           also install the Claude Code skill/agent/hook for all projects
#   ./install.sh --claude project        ...or just for the git repo you're in (commit .claude/ to share)
#   ./install.sh --git-hooks             also keep the graph warm via git hooks in the current repo
#   ./install.sh --prefix ~/.local       install the binary to ~/.local/bin (the default)
#   ./install.sh --from-source           build with cargo from this checkout instead of downloading
#   ./install.sh --uninstall [--claude ...] [--git-hooks]
#
# Prebuilt binaries come from GitHub Releases (macOS, Linux, Windows; x86_64 and arm64).
set -eu

REPO=grepum/grepum
SRC_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || pwd)
CLAUDE=""
GIT_HOOKS=0
PREFIX=""
UNINSTALL=0
YES=0
FROM_SOURCE=0

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --claude) [ $# -ge 2 ] || die "--claude needs user|project|none"; CLAUDE=$2; shift 2 ;;
        --claude=*) CLAUDE=${1#*=}; shift ;;
        --git-hooks) GIT_HOOKS=1; shift ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX=$2; shift 2 ;;
        --prefix=*) PREFIX=${1#*=}; shift ;;
        --from-source) FROM_SOURCE=1; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        -y|--yes) YES=1; shift ;;
        -h|--help) usage ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done
case "$CLAUDE" in ""|user|project|none) ;; *) die "--claude must be user, project or none" ;; esac

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
if [ -n "$PREFIX" ]; then
    BIN_DIR="$PREFIX/bin"
elif [ "$FROM_SOURCE" = 1 ]; then
    BIN_DIR="$CARGO_BIN"
else
    BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
fi
BIN="$BIN_DIR/grepum"

in_git_repo() { git rev-parse --is-inside-work-tree >/dev/null 2>&1; }

# Ask on the terminal even when the script itself is piped into sh.
ask_claude_scope() {
    if [ "$YES" = 1 ]; then CLAUDE=user; return; fi
    if ! (exec </dev/tty) 2>/dev/null; then CLAUDE=none; return; fi
    printf 'Set up Claude Code (skill + code-navigator agent + session hook)?\n  [u]ser: all projects (~/.claude)\n  [p]roject: this repo only (./.claude)\n  [n]o\nChoice [u]: ' >/dev/tty
    read -r answer </dev/tty || answer=""
    case "$answer" in
        ""|u|U|user) CLAUDE=user ;;
        p|P|project) CLAUDE=project ;;
        *) CLAUDE=none ;;
    esac
}

# ---------------------------------------------------------------- uninstall
if [ "$UNINSTALL" = 1 ]; then
    if [ -x "$BIN" ]; then
        case "$CLAUDE" in
            ""|user) "$BIN" uninstall claude --scope user ;;
            project) "$BIN" uninstall claude --scope project ;;
        esac
        if [ "$GIT_HOOKS" = 1 ] && in_git_repo; then "$BIN" uninstall git-hooks; fi
    fi
    if [ -x "$BIN" ] && [ "$BIN_DIR" != "$CARGO_BIN" ]; then
        rm -f "$BIN" && say "removed $BIN"
        rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/grepum"   # install receipt
    elif command -v cargo >/dev/null 2>&1 && cargo uninstall grepum >/dev/null 2>&1; then
        say "removed $CARGO_BIN/grepum"
    else
        say "no grepum binary found at $BIN"
    fi
    say "Per-repository caches live in <repo>/.git/grepum; delete them with 'grepum clear' or rm -rf."
    exit 0
fi

# ---------------------------------------------------------------- install
command -v git >/dev/null 2>&1 || die "git is required"

if [ "$FROM_SOURCE" = 1 ]; then
    command -v cargo >/dev/null 2>&1 || die "--from-source needs Rust's cargo (https://rustup.rs)"
    [ -f "$SRC_DIR/Cargo.toml" ] || die "--from-source must be run from a grepum source checkout"
    say "Building and installing grepum into $BIN_DIR ..."
    if [ -n "$PREFIX" ]; then
        cargo install --quiet --locked --force --path "$SRC_DIR" --root "$PREFIX"
    else
        cargo install --quiet --locked --force --path "$SRC_DIR"
    fi
else
    command -v curl >/dev/null 2>&1 || die "curl is required (or use --from-source)"
    say "Downloading the latest grepum release into $BIN_DIR ..."
    mkdir -p "$BIN_DIR"
    TMP=$(mktemp)
    trap 'rm -f "$TMP"' EXIT
    curl --proto '=https' --tlsv1.2 -fLsS -o "$TMP" "https://github.com/$REPO/releases/latest/download/grepum-installer.sh" \
        || die "download failed; build from source instead: git clone https://github.com/$REPO && cd grepum && ./install.sh --from-source"
    GREPUM_INSTALL_DIR="$BIN_DIR" GREPUM_NO_MODIFY_PATH=1 sh "$TMP" --quiet || die "the release installer failed"
fi
"$BIN" --version >/dev/null 2>&1 || die "install finished but $BIN does not run"
say "  installed $("$BIN" --version)"

FOUND=$(command -v grepum 2>/dev/null || true)
if [ "$FOUND" != "$BIN" ]; then
    say ""
    if [ -z "$FOUND" ]; then
        say "  note: $BIN_DIR is not on your PATH. Add this to your shell profile:"
    else
        say "  note: another grepum ($FOUND) shadows the new one. Put this first on your PATH:"
    fi
    say "    export PATH=\"$BIN_DIR:\$PATH\""
fi

[ -n "$CLAUDE" ] || ask_claude_scope
case "$CLAUDE" in
    user) say ""; "$BIN" install claude --scope user ;;
    project)
        say ""
        in_git_repo || die "--claude project must be run from inside the target git repository"
        "$BIN" install claude --scope project ;;
esac

if [ "$GIT_HOOKS" = 1 ]; then
    say ""
    in_git_repo || die "--git-hooks must be run from inside the target git repository"
    "$BIN" install git-hooks
fi

say ""
say "Done. Try it in any git repository:"
say "  grepum summary"
say "  grepum find <words>     grepum callers <Symbol>     grepum diff main"
