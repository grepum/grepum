#!/usr/bin/env python3
"""Benchmark grepum against graphify.

    cargo build --release
    python3 benchmark/generate.py          # synthetic projects with known call graphs
    python3 benchmark/bench.py --graphify /path/to/graphify [--set synthetic|real|all] [--reps 3]

Two project sets:
  synthetic   benchmark/projects/* from generate.py; scored against ground truth
  real        open-source repos from real_repos.json, shallow-cloned at a pinned
              tag into benchmark/repos/ (gitignored); speed and disk only

Results go to benchmark/results/<set>.{md,json} and, with the machine, commit
and tool versions recorded, to benchmark/results/runs/<timestamp>-<set>.{md,json}.

Each tool gets its own fresh copy of each project (a git repo with four
feature branches) so neither sees the other's output. Scenarios mirror a
worktree-heavy agent workflow:

  cold          first index of the main checkout (all caches empty)
  warm          re-index with nothing changed
  edit          one source file edited (uncommitted), re-index
  worktrees     `git worktree add` 4 feature branches, index each one
  query         "who calls X" from a fresh process
  other rev     graph of a branch that is NOT checked out anywhere
  accuracy      call edges vs. the generator's ground truth

graphify runs as `graphify update .` (AST only, no LLM), which is what its
docs recommend for code. grepum runs as `grepum index`.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import platform
import shutil
import statistics
import subprocess
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
GREPUM = HERE.parent / "target" / "release" / "grepum"
FEATURES = 4

QUERY = {"shopfront": ("OrderService.get", "OrderService.get"), "datapipe": ("Metrics.incr", "Metrics.incr"),
         "arena": ("UHealthComponent::ApplyDamage", "UHealthComponent::ApplyDamage")}

REAL = {r["name"]: r for r in json.loads((HERE / "real_repos.json").read_text())["repos"]}
SYNTHETIC = ["shopfront", "datapipe", "arena"]

EDITS = {
    "shopfront": ("src/lib/money.ts", "\nexport function feature{n}(x: number): string {{\n  return formatMoney(addTax(x, 0.{n}));\n}}\n"),
    "datapipe": ("datapipe/utils/text.py", "\n\ndef feature{n}(value: str) -> str:\n    return truncate(tokenize(value)[0])\n"),
    "arena": ("Source/Arena/Private/ArenaDamage.cpp",
              "\nnamespace ArenaDamage\n{{\n\tfloat Feature{n}(float X)\n\t{{\n\t\treturn Scale(X, 0.{n}f);\n\t}}\n}}\n"),
    **{name: (r["edit_file"], r["edit"]) for name, r in REAL.items()},
}
QUERY.update({name: (r["query"], r["query"]) for name, r in REAL.items()})


def sh(args, cwd, env=None, check=True) -> float:
    t = time.perf_counter()
    p = subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    dt = time.perf_counter() - t
    if check and p.returncode != 0:
        raise RuntimeError(f"{args} failed in {cwd}:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}")
    return dt


def out(args, cwd) -> str:
    return subprocess.run(args, cwd=cwd, check=True, stdout=subprocess.PIPE, text=True).stdout


def git(cwd, *args):
    subprocess.run(["git", "-c", "user.email=bench@example.com", "-c", "user.name=bench", *args], cwd=cwd, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def du(path: Path) -> int:
    if not path.exists():
        return 0
    return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())


def fetch_real(name: str) -> Path:
    """Shallow clone of a pinned tag, reused across runs."""
    r = REAL[name]
    path = HERE / "repos" / name
    if not path.exists():
        print(f"cloning {r['url']} @ {r['rev']} ...", flush=True)
        path.parent.mkdir(exist_ok=True)
        subprocess.run(["git", "-c", "advice.detachedHead=false", "clone", "-q", "--depth", "1", "--branch", r["rev"],
                        r["url"], str(path)], check=True)
    return path


def source_of(project: str) -> Path:
    if project in REAL:
        return fetch_real(project)
    src = HERE / "projects" / project
    if not src.exists():
        raise SystemExit(f"{src} is missing; run: python3 benchmark/generate.py")
    return src


def make_repo(project: str, dest: Path) -> Path:
    main = dest / "main"
    shutil.copytree(source_of(project), main, ignore=shutil.ignore_patterns(".git"))
    git(main, "init", "-q", "-b", "main")
    git(main, "add", "-A")
    git(main, "commit", "-qm", "initial import")
    rel, tmpl = EDITS[project]
    for i in range(1, FEATURES + 1):
        git(main, "checkout", "-q", "-b", f"feature-{i}")
        with open(main / rel, "a") as fh:
            fh.write(tmpl.format(n=10 + i))
        git(main, "commit", "-qam", f"feature {i}")
        git(main, "checkout", "-q", "main")
    return main


class Tool:
    name = ""

    def index(self, cwd: Path) -> float: ...
    def clear(self, cwd: Path) -> None: ...
    def query(self, cwd: Path, sym: str) -> float: ...
    def disk(self, repo_root: Path) -> int: ...
    def edges(self, cwd: Path) -> set[tuple[str, str]]: ...


class Grepum(Tool):
    name = "grepum"

    def index(self, cwd):
        return sh([str(GREPUM), "index", "--quiet"], cwd)

    def clear(self, cwd):
        sh([str(GREPUM), "clear"], cwd)

    def query(self, cwd, sym):
        return sh([str(GREPUM), "callers", sym, "--json"], cwd)

    def other_rev(self, cwd, rev):
        return sh([str(GREPUM), "index", "--quiet", "--rev", rev], cwd)

    def disk(self, repo_root):
        return du(repo_root / "main" / ".git" / "grepum") + sum(du(p / "grepum") for p in (repo_root / "main" / ".git" / "worktrees").glob("*"))

    def edges(self, cwd):
        g = json.loads(out([str(GREPUM), "export"], cwd))
        defs = {d["id"]: f'{d["file"]}:{d["line"]}' for d in g["defs"]}
        return {(defs[e["from"]], defs[e["to"]]) for e in g["edges"] if e["kind"] == "call" and e["from"] is not None}


class Graphify(Tool):
    name = "graphify"

    def __init__(self, binary: str):
        self.bin = binary
        self.env = {k: v for k, v in os.environ.items() if not k.endswith("_API_KEY")}

    def index(self, cwd):
        return sh([self.bin, "update", "."], cwd, env=self.env)

    def clear(self, cwd):
        shutil.rmtree(cwd / "graphify-out", ignore_errors=True)

    def query(self, cwd, sym):
        return sh([self.bin, "affected", sym, "--depth", "1"], cwd, env=self.env, check=False)

    def disk(self, repo_root):
        return sum(du(p / "graphify-out") for p in repo_root.iterdir() if p.is_dir())

    def edges(self, cwd):
        g = json.loads((cwd / "graphify-out" / "graph.json").read_text())
        loc = {}
        for n in g["nodes"]:
            sf, sl = n.get("source_file"), n.get("source_location")
            if not sf or not sl or n.get("label") == os.path.basename(sf):
                continue  # skip file nodes
            loc[n["id"]] = f'{sf}:{sl.lstrip("L").split("-")[0]}'
        res = set()
        for e in g.get("links", g.get("edges", [])):
            if e.get("relation") == "calls" and e["source"] in loc and e["target"] in loc:
                res.add((loc[e["source"]], loc[e["target"]]))
        return res


def score(edges: set[tuple[str, str]], truth: dict) -> dict:
    callers = {f'{d["file"]}:{d["line"]}' for d in truth["defs"] if d["kind"] in ("fn", "method")}
    alldefs = {f'{d["file"]}:{d["line"]}' for d in truth["defs"]}
    gold = {(c["caller"], c["callee"]) for c in truth["calls"]}
    hard = {(c["caller"], c["callee"]): c["hard"] for c in truth["calls"] if c["hard"]}
    alias = truth.get("aliases", {})  # C++ header prototype -> its definition: linking either is right
    edges = {(alias.get(a, a), alias.get(b, b)) for a, b in edges}
    pred = {e for e in edges if e[0] in callers and e[1] in alldefs}
    tp = len(pred & gold)
    p = tp / len(pred) if pred else 0.0
    r = tp / len(gold) if gold else 0.0
    hard_hit = {}
    for k, kind in hard.items():
        hard_hit.setdefault(kind, [0, 0])
        hard_hit[kind][1] += 1
        if k in pred:
            hard_hit[kind][0] += 1
    return {"precision": p, "recall": r, "f1": 2 * p * r / (p + r) if p + r else 0.0, "tp": tp,
            "fp": len(pred - gold), "fn": len(gold - pred), "hard": hard_hit,
            "fp_examples": sorted(pred - gold)[:5], "fn_examples": sorted(gold - pred)[:5]}


def median(xs):
    return statistics.median(xs) if xs else float("nan")


def bench_tool(tool: Tool, project: str, work: Path, reps: int) -> dict:
    root = work / f"{project}-{tool.name}"
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    main = make_repo(project, root)
    r: dict = {}

    cold = []
    for _ in range(reps):
        tool.clear(main)
        cold.append(tool.index(main))
    r["cold"] = median(cold)
    r["warm"] = median([tool.index(main) for _ in range(reps + 2)])

    rel, tmpl = EDITS[project]
    edits = []
    for k in range(reps):
        with open(main / rel, "a") as fh:
            fh.write(tmpl.format(n=50 + k))
        edits.append(tool.index(main))
    r["edit"] = median(edits)
    git(main, "checkout", "-q", "--", rel)
    tool.index(main)

    wt_times = []
    for i in range(1, FEATURES + 1):
        wt = root / f"wt-{i}"
        git(main, "worktree", "add", "-q", str(wt), f"feature-{i}")
        wt_times.append(tool.index(wt))
    r["worktree_each"] = wt_times
    r["worktrees_total"] = sum(wt_times)

    sym = QUERY[project][0 if tool.name == "grepum" else 1]
    r["query"] = median([tool.query(main, sym) for _ in range(reps + 2)])
    if isinstance(tool, Grepum):
        r["other_rev"] = tool.other_rev(main, "feature-2^")  # a commit no worktree has checked out
    r["disk"] = tool.disk(root)
    edges = tool.edges(main)
    if project in REAL:
        r["call_edges"] = len(edges)  # no ground truth for real code
    else:
        truth = json.loads((HERE / "truth" / f"{project}.json").read_text())
        r["accuracy"] = score(edges, truth)
    return r


def fmt_s(x):
    return f"{x * 1000:.0f} ms" if x < 1 else f"{x:.2f} s"


def report(results: dict) -> str:
    lines = []
    for project, by_tool in results.items():
        w, g = by_tool["grepum"], by_tool["graphify"]
        if project in REAL:
            r = REAL[project]
            lines.append(f"### {project} ({r['lang']}, [{r['url'].removesuffix('.git').split('github.com/')[1]}]"
                         f"({r['url'].removesuffix('.git')}) @ {r['rev']})\n")
        else:
            lines.append(f"### {project} (synthetic)\n")
        lines.append("| scenario | grepum | graphify | speedup |")
        lines.append("|---|---:|---:|---:|")
        for key, label in [("cold", "cold index (main checkout)"), ("warm", "re-index, nothing changed"),
                           ("edit", "re-index after editing 1 file"), ("worktrees_total", f"index {FEATURES} new worktrees (total)"),
                           ("query", "who-calls query (new process)")]:
            lines.append(f"| {label} | {fmt_s(w[key])} | {fmt_s(g[key])} | {g[key] / w[key]:.1f}x |")
        lines.append(f"| graph of an un-checked-out commit | {fmt_s(w['other_rev'])} | needs a checkout + full index | |")
        lines.append(f"| disk used (main + {FEATURES} worktrees) | {w['disk'] / 1024:.0f} KiB | {g['disk'] / 1024:.0f} KiB | {g['disk'] / max(w['disk'], 1):.1f}x less |")
        if project in REAL:
            lines.append(f"| call edges reported (no ground truth) | {w['call_edges']} | {g['call_edges']} | |")
            lines.append("")
            continue
        wa, ga = w["accuracy"], g["accuracy"]
        lines.append(f"| call-edge precision | {wa['precision']:.1%} | {ga['precision']:.1%} | |")
        lines.append(f"| call-edge recall | {wa['recall']:.1%} | {ga['recall']:.1%} | |")
        lines.append(f"| F1 | {wa['f1']:.3f} | {ga['f1']:.3f} | |")
        for kind in sorted(set(wa["hard"]) | set(ga["hard"])):
            a = wa["hard"].get(kind, [0, 0])
            b = ga["hard"].get(kind, [0, 0])
            lines.append(f"| hard case `{kind}` found | {a[0]}/{a[1]} | {b[0]}/{b[1]} | |")
        lines.append("")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--graphify", default=os.environ.get("GRAPHIFY", "graphify"))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--work", default="/tmp/grepum-bench")
    ap.add_argument("--set", choices=["synthetic", "real", "all"], default="all")
    ap.add_argument("--projects", nargs="*", help="run only these projects (names from either set)")
    a = ap.parse_args()
    a.graphify = shutil.which(a.graphify) or str(Path(a.graphify).resolve())
    if not GREPUM.exists():
        raise SystemExit("build first: cargo build --release")
    work = Path(a.work)
    sets = {"synthetic": SYNTHETIC, "real": list(REAL)}
    if a.set != "all":
        sets = {a.set: sets[a.set]}
    if a.projects:
        sets = {k: [p for p in v if p in a.projects] for k, v in sets.items()}
    meta = run_meta(a)
    header = (f"Run {meta['date']} on {meta['machine']}; grepum {meta['grepum']} ({meta['commit']}), "
              f"{meta['graphify']}; median of {a.reps} runs.\n\n")
    stamp = meta["date"].replace(":", "").replace("-", "")[:15]
    runs = HERE / "results" / "runs"
    runs.mkdir(parents=True, exist_ok=True)
    for set_name, projects in sets.items():
        if not projects:
            continue
        results = {}
        for project in projects:
            results[project] = {}
            for tool in (Grepum(), Graphify(a.graphify)):
                print(f"[{project}] {tool.name} ...", flush=True)
                results[project][tool.name] = bench_tool(tool, project, work, a.reps)
        md = report(results)
        print()
        print(md)
        out_dir = HERE / "results"
        (out_dir / f"{set_name}.json").write_text(json.dumps(results, indent=1, default=str))
        (out_dir / f"{set_name}.md").write_text(header + md + "\n")
        (runs / f"{stamp}-{set_name}.json").write_text(
            json.dumps({"meta": meta, "results": results}, indent=1, default=str))
        (runs / f"{stamp}-{set_name}.md").write_text(header + md + "\n")
        print(f"saved benchmark/results/{set_name}.md and benchmark/results/runs/{stamp}-{set_name}.md")


def first_line(cmd, cwd=None) -> str:
    try:
        out = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=30).stdout
        return out.strip().splitlines()[0] if out.strip() else "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def run_meta(a) -> dict:
    cpu = platform.processor() or platform.machine()
    if platform.system() == "Darwin":
        cpu = first_line(["sysctl", "-n", "machdep.cpu.brand_string"])
    elif Path("/proc/cpuinfo").exists():
        cpu = next((l.split(":", 1)[1].strip() for l in Path("/proc/cpuinfo").read_text().splitlines()
                    if l.startswith("model name")), cpu)
    commit = first_line(["git", "rev-parse", "--short", "HEAD"], cwd=HERE)
    if first_line(["git", "status", "--porcelain", "--untracked-files=no", "--", "../src"], cwd=HERE) != "unknown":
        commit += "-dirty"
    return {
        "date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "machine": f"{cpu}, {platform.system()} {platform.release()}",
        "grepum": first_line([str(GREPUM), "--version"]).removeprefix("grepum "),
        "commit": commit,
        "graphify": first_line([a.graphify, "--version"]),
        "reps": a.reps,
    }


if __name__ == "__main__":
    main()
