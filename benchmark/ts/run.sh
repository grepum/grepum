#!/bin/sh
# Score grepum's TypeScript call graph against the TypeScript compiler.
#
#   benchmark/ts/run.sh [path/to/grepum]     # default: target/release/grepum
#
# Clones hono and excalidraw at pinned tags into $WORK (nothing is committed
# here), asks tsc's type checker what every call, `new` and <Component/>
# resolves to (tsc_calls.js), and compares that with `grepum export`
# (compare.py). Needs git, node, npm and python3.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
GREPUM=$(cd "$(dirname "${1:-$HERE/../../target/release/grepum}")" && pwd)/$(basename "${1:-grepum}")
WORK=${WORK:-/tmp/grepum-ts-bench}
TS_VERSION=5.9.3
mkdir -p "$WORK/tools" "$WORK/repos"
[ -d "$WORK/tools/node_modules/typescript" ] || (cd "$WORK/tools" && npm install --silent --no-save "typescript@$TS_VERSION")
export NODE_PATH="$WORK/tools/node_modules"

clone() { # name url tag
    [ -d "$WORK/repos/$1" ] || git clone -q --depth 1 --branch "$3" "$2" "$WORK/repos/$1"
}
clone hono https://github.com/honojs/hono.git v4.13.19
clone excalidraw https://github.com/excalidraw/excalidraw.git v0.18.0

score() { # name tsconfig src-dirs...
    name=$1; tsconfig=$2; shift 2
    [ -s "$WORK/$name.sites.jsonl" ] || node "$HERE/tsc_calls.js" "$WORK/repos/$name" "$tsconfig" "$@" > "$WORK/$name.sites.jsonl"
    (cd "$WORK/repos/$name" && "$GREPUM" index --rebuild >/dev/null && "$GREPUM" export > "$WORK/$name.grepum.json")
    echo "== $name"
    python3 "$HERE/compare.py" "$WORK/$name.grepum.json" "$WORK/$name.sites.jsonl" --dump "$WORK/$name.dump" | sed -n 1,5p
}
score hono tsconfig.json src
score excalidraw tsconfig.json packages excalidraw-app
echo "misses and wrong edges: $WORK/<repo>.dump/{miss,fp}.jsonl"
