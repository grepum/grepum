#!/usr/bin/env python3
"""What an agent reads to answer code-navigation questions: grepum vs plain grep.

    cargo build --release
    python3 benchmark/generate.py            # synthetic projects with known call graphs
    python3 benchmark/vs_grep.py [--sample 50] [--set synthetic|real|all] [--tsc hono=/path/hono.sites.jsonl]

The baseline is the plain search an agent runs today, `git grep -n -w <name>`
(same matches and output as `rg -n -w` on tracked files), followed by reading
what it found the way an agent's file-read tool does: the whole file, up to
2,000 lines. Each question is answered both ways and we count what lands in
the agent's context. Tokens are estimated as bytes / 4 for both sides, and
file reads are counted without the line numbers a read tool adds, which
favours grep.

Questions, asked about sampled definitions of every project:

  who calls X       grepum callers X                vs  git grep -n -w X
  ...with the code  grepum callers X | grepum show -  vs  git grep, then read each file it hit
  read X            grepum show path:line           vs  git grep to find X, then read its file
                                                       (also: read a 60-line window from X)
  what's in file F  grepum outline F                vs  read F

"Who calls X" is also scored for correctness where the true callers are
known: the synthetic projects record every call they make, and for hono the
TypeScript compiler resolves every call (benchmark/ts/tsc_calls.js; run.sh
writes it). grep's answer is the function around each matching line, the
best it can say without reading, after dropping the lines anyone discards at
a glance: definitions, imports and comments. Reading can filter more false
hits, but can't add the calls grep didn't match.

Results go to benchmark/results/vs-grep.{md,json} and runs/<timestamp>-vs-grep.json.
"""
from __future__ import annotations

import argparse
import bisect
import collections
import json
import random
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

import bench

HERE = Path(__file__).resolve().parent
GREPUM = str(bench.GREPUM)
READ_LIMIT = 2000      # lines a file-read tool returns by default
WINDOW = 60            # lines a careful agent reads from a definition
TEST = re.compile(r"(\.test\.|\.spec\.|__tests__|/tests?/|/mocks?/|/fixtures?/|benchmarks?/|\.d\.ts$)")
IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
CALLABLE = {"fn", "method"}


def tokens(n_bytes: int) -> int:
    return round(n_bytes / 4)


def run(args, cwd, stdin=None) -> tuple[str, float]:
    t = time.perf_counter()
    p = subprocess.run(args, cwd=cwd, input=stdin, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    return p.stdout, time.perf_counter() - t


class Project:
    def __init__(self, name: str, lang: str, path: Path, truth: dict | None):
        self.name, self.lang, self.path = name, lang, path
        subprocess.run([GREPUM, "index", "--quiet"], cwd=path, check=True)
        g = json.loads(run([GREPUM, "export"], path)[0])
        self.defs = g["defs"]
        self.by_loc = {f'{d["file"]}:{d["line"]}': d for d in self.defs}
        self.by_file: dict[str, list] = collections.defaultdict(list)
        for d in self.defs:
            self.by_file[d["file"]].append(d)
        self.files = sorted({f["path"] for f in g["files"]})
        self.truth = truth  # target loc -> set of caller locs, or None
        self.alias: dict[str, str] = {}
        self.skip_tests = False
        self._sizes: dict[str, int] = {}

    def caller_of(self, file: str, line: int, kinds=CALLABLE | {"class"}) -> str | None:
        best = None
        for d in self.by_file.get(file, []):
            if d["kind"] in kinds and d["line"] <= line <= d["end_line"]:
                if best is None or d["end_line"] - d["line"] <= best["end_line"] - best["line"]:
                    best = d
        return f'{best["file"]}:{best["line"]}' if best else None

    def read_bytes(self, file: str, start: int = 1, limit: int = READ_LIMIT) -> int:
        """Bytes a read tool returns for `limit` lines from `start`."""
        try:
            lines = (self.path / file).read_bytes().split(b"\n")
        except OSError:
            return 0
        return sum(len(l) + 1 for l in lines[start - 1:start - 1 + limit])

    def norm(self, loc: str) -> str:
        return self.alias.get(loc, loc)


# ---------- questions ----------

NOT_A_CALL = re.compile(r"^\s*(import\b|from\s+\S+\s+import\b|use\b|#\s*include\b|export\s*\{|//|#|/\*|\*)")


def grep(p: Project, name: str) -> tuple[str, float, list[tuple[str, int]]]:
    """`git grep -n -w name`, and the hits that may be calls: not a definition
    of that name, an import or a comment line."""
    text, dt = run(["git", "grep", "-n", "-w", "-I", "--no-color", "-e", name], p.path)
    defined = {(d["file"], d["line"]) for d in p.defs if d["name"] == name}
    hits = []
    for l in text.splitlines():
        parts = l.split(":", 2)
        if len(parts) == 3 and parts[1].isdigit():
            hit = (parts[0], int(parts[1]))
            if hit not in defined and not NOT_A_CALL.match(parts[2]):
                hits.append(hit)
    return text, dt, hits


def who_calls(p: Project, target: dict) -> dict:
    loc = f'{target["file"]}:{target["line"]}'
    sym = f'{target["file"]}:{target["qual"]}'
    keep = (lambda f: not TEST.search(f)) if p.skip_tests else (lambda f: True)

    text, g_dt = run([GREPUM, "callers", sym], p.path)
    js = json.loads(run([GREPUM, "--json", "callers", sym], p.path)[0] or "{}")
    # Every answer is attributed the same way: the function around the call line.
    g_ans = set()
    for h in js.get("callers", []):
        f = h["at"].rsplit(":", 1)[0]
        c = p.caller_of(f, h["call_line"]) if keep(f) else None
        if c is not None:
            g_ans.add(p.norm(c))
    shown, _ = run([GREPUM, "show", "-"], p.path, stdin=text)

    gtext, r_dt, hits = grep(p, target["name"])
    own = {(target["file"], target["line"])}
    own |= {tuple(k.rsplit(":", 1)) for k, v in p.alias.items() if v == loc}
    own = {(f, int(l)) for f, l in own}
    r_ans = set()
    for f, l in hits:
        if (f, l) in own or not keep(f):
            continue
        c = p.caller_of(f, l)
        if c is not None:
            r_ans.add(p.norm(c))
    hit_files = sorted({l.split(":", 1)[0] for l in gtext.splitlines() if ":" in l})
    read = sum(p.read_bytes(f) for f in hit_files)
    return {
        "truth": p.truth.get(loc) if p.truth is not None else None,
        "grepum": {"answer": g_ans, "tokens": tokens(len(text)), "code_tokens": tokens(len(shown)), "s": g_dt, "calls": 1, "code_calls": 1},
        "grep": {"answer": r_ans, "tokens": tokens(len(gtext)), "code_tokens": tokens(len(gtext) + read), "s": r_dt,
                 "calls": 1, "code_calls": 1 + len(hit_files)},
    }


def read_function(p: Project, d: dict) -> dict:
    out, g_dt = run([GREPUM, "show", f'{d["file"]}:{d["line"]}'], p.path)
    length = d["end_line"] - d["line"] + 1
    gtext, r_dt, _ = grep(p, d["name"])
    return {
        "grepum": {"tokens": tokens(len(out)), "complete": length <= 200, "s": g_dt, "calls": 1},
        "grep": {"tokens": tokens(len(gtext) + p.read_bytes(d["file"])), "complete": length <= READ_LIMIT, "s": r_dt, "calls": 2},
        "grep_window": {"tokens": tokens(len(gtext) + p.read_bytes(d["file"], d["line"], WINDOW)), "complete": length <= WINDOW, "calls": 2},
    }


def whats_in(p: Project, file: str) -> dict:
    out, g_dt = run([GREPUM, "outline", file], p.path)
    return {"grepum": {"tokens": tokens(len(out)), "s": g_dt, "calls": 1},
            "grep": {"tokens": tokens(p.read_bytes(file)), "calls": 1}}


# ---------- projects and ground truth ----------

def synthetic(name: str, lang: str, work: Path) -> Project:
    src = HERE / "projects" / name
    if not src.exists():
        raise SystemExit(f"{src} is missing; run: python3 benchmark/generate.py")
    dest = work / name
    shutil.copytree(src, dest, ignore=shutil.ignore_patterns(".git"))
    bench.git(dest, "init", "-q", "-b", "main")
    bench.git(dest, "add", "-A")
    bench.git(dest, "commit", "-qm", "import")
    t = json.loads((HERE / "truth" / f"{name}.json").read_text())
    alias = t.get("aliases", {})
    truth: dict[str, set] = collections.defaultdict(set)
    for c in t["calls"]:
        truth[alias.get(c["callee"], c["callee"])].add(alias.get(c["caller"], c["caller"]))
    p = Project(name, lang, dest, dict(truth))
    p.alias = alias
    return p


def tsc_truth(p: Project, sites_path: Path) -> dict:
    """Callers of each project function per tsc, as compare.py maps them (tests left out)."""
    def gdef_for(decl):
        cands = [d for d in p.by_file.get(decl["file"], []) if d["name"] == decl["name"]]
        inside = [d for d in cands if d["line"] <= decl["line"] <= d["end_line"]]
        best = min(inside, key=lambda d: d["end_line"] - d["line"], default=None)
        if best is None:
            near = [d for d in cands if abs(d["line"] - decl["line"]) <= 3]
            best = near[0] if near else None
        if best is None:
            return None
        same = [d for d in p.by_file[decl["file"]] if d["qual"] == best["qual"]]
        return min(same, key=lambda d: d["line"])

    truth: dict[str, set] = collections.defaultdict(set)
    for line in sites_path.read_text().splitlines():
        s = json.loads(line)
        if s["status"] != "project" or not s["decl"]["eligible"] or TEST.search(s["file"]) or TEST.search(s["decl"]["file"]):
            continue
        t = gdef_for(s["decl"])
        c = p.caller_of(s["file"], s["line"])
        if t is not None and c is not None:
            truth[f'{t["file"]}:{t["line"]}'].add(c)
    return dict(truth)


# ---------- scoring ----------

def summarize(rows: list[dict], tools: list[str], keys: list[str]) -> dict:
    out = {}
    for t in tools:
        out[t] = {}
        for k in keys:
            vals = [r[t][k] for r in rows if k in r[t]]
            if not vals:
                continue
            if isinstance(vals[0], bool):
                out[t][k] = sum(vals) / len(vals)
            elif k == "s":
                out[t]["ms"] = statistics.median(vals) * 1000
            else:
                out[t][k] = statistics.median(vals)
                out[t][k + "_total"] = sum(vals)
    return out


def accuracy(rows: list[dict], tool: str) -> dict:
    """Pooled over all questions, and per question (the typical question)."""
    tp = fp = fn = 0
    per_p, per_r = [], []
    for r in rows:
        truth, ans = r["truth"] or set(), r[tool]["answer"]
        tp += len(ans & truth)
        fp += len(ans - truth)
        fn += len(truth - ans)
        if ans:
            per_p.append(len(ans & truth) / len(ans))
        if truth:
            per_r.append(len(ans & truth) / len(truth))
    return {"recall": tp / (tp + fn) if tp + fn else 0.0, "precision": tp / (tp + fp) if tp + fp else 0.0,
            "recall_per_question": statistics.mean(per_r) if per_r else 0.0,
            "precision_per_question": statistics.mean(per_p) if per_p else 0.0,
            "tp": tp, "fp": fp, "fn": fn}


def bench_project(p: Project, sample: int, rng: random.Random) -> dict:
    callables = [d for d in p.defs if d["kind"] in CALLABLE and IDENT.match(d["name"]) and not d.get("decl")]
    res: dict = {"lang": p.lang}

    # who calls X: scored targets where the truth is known, else any function with callers in the graph
    if p.truth is not None:
        pool = [p.by_loc[l] for l in sorted(p.truth) if l in p.by_loc and IDENT.match(p.by_loc[l]["name"])]
    else:
        pool = callables
    targets = rng.sample(pool, min(sample, len(pool)))
    rows = [who_calls(p, d) for d in targets]
    res["callers"] = summarize(rows, ["grepum", "grep"], ["tokens", "code_tokens", "calls", "code_calls", "s"])
    res["callers"]["n"] = len(rows)
    if p.truth is not None:
        res["callers"]["accuracy"] = {t: accuracy(rows, t) for t in ("grepum", "grep")}

    funcs = rng.sample(callables, min(sample, len(callables)))
    rows = [read_function(p, d) for d in funcs]
    res["show"] = summarize(rows, ["grepum", "grep", "grep_window"], ["tokens", "complete", "calls", "s"])
    res["show"]["n"] = len(rows)

    files = [f for f in p.files if p.by_file.get(f)]
    files = rng.sample(files, min(sample, len(files)))
    rows = [whats_in(p, f) for f in files]
    res["outline"] = summarize(rows, ["grepum", "grep"], ["tokens", "calls", "s"])
    res["outline"]["n"] = len(rows)
    return res


# ---------- report ----------

def ratio(a, b):
    return f"{b / a:.0f}×" if a and b / a >= 10 else (f"{b / a:.1f}×" if a else "-")


def report(results: dict) -> str:
    L = ["## Tokens an agent reads per question (median)", "",
         "| project | who calls X: grepum / grep | ...with their code: grepum / grep + read | read X: grepum / grep + read (complete) | read X, 60-line window (complete) | what's in a file: grepum / read |",
         "|---|---|---|---|---|---|"]
    for name, r in results.items():
        c, s, o = r["callers"], r["show"], r["outline"]
        L.append(
            f"| {name} ({r['lang']}) "
            f"| {c['grepum']['tokens']:.0f} / {c['grep']['tokens']:.0f} ({ratio(c['grepum']['tokens'], c['grep']['tokens'])}) "
            f"| {c['grepum']['code_tokens']:.0f} / {c['grep']['code_tokens']:.0f} ({ratio(c['grepum']['code_tokens'], c['grep']['code_tokens'])}; "
            f"{c['grep']['code_calls']:.0f} tool calls vs 1) "
            f"| {s['grepum']['tokens']:.0f} ({s['grepum']['complete']:.0%}) / {s['grep']['tokens']:.0f} ({s['grep']['complete']:.0%}) "
            f"({ratio(s['grepum']['tokens'], s['grep']['tokens'])}) "
            f"| {s['grep_window']['tokens']:.0f} ({s['grep_window']['complete']:.0%}) "
            f"| {o['grepum']['tokens']:.0f} / {o['grep']['tokens']:.0f} ({ratio(o['grepum']['tokens'], o['grep']['tokens'])}) |")
    L += ["", "Complete: share of sampled functions shown in full (grepum shows up to 200 lines, then the start and end).", "",
          "## Who calls X: correctness", "",
          "| project | truth | grepum recall | grepum precision | grep recall | grep precision | precision per question: grepum / grep |",
         "|---|---|---|---|---|---|---|"]
    for name, r in results.items():
        a = r["callers"].get("accuracy")
        if a:
            src = "TypeScript compiler" if name == "hono" else "generator"
            L.append(f"| {name} | {src} | {a['grepum']['recall']:.1%} | {a['grepum']['precision']:.1%} "
                     f"| {a['grep']['recall']:.1%} | {a['grep']['precision']:.1%} "
                     f"| {a['grepum']['precision_per_question']:.1%} / {a['grep']['precision_per_question']:.1%} |")
    L += ["", "## Median wall time per question", "", "| project | who calls X: grepum / grep | read X: grepum / grep | outline |", "|---|---|---|---|"]
    for name, r in results.items():
        c, s, o = r["callers"], r["show"], r["outline"]
        L.append(f"| {name} | {c['grepum']['ms']:.0f} ms / {c['grep']['ms']:.0f} ms | {s['grepum']['ms']:.0f} ms / {s['grep']['ms']:.0f} ms "
                 f"| {o['grepum']['ms']:.0f} ms |")
    return "\n".join(L) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--set", choices=["synthetic", "real", "all"], default="all")
    ap.add_argument("--sample", type=int, default=50, help="questions per project and kind")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--tsc", action="append", default=[], metavar="REPO=SITES",
                    help="tsc call sites (from benchmark/ts) to score a real repo's callers")
    a = ap.parse_args()
    tsc = dict(x.split("=", 1) for x in a.tsc)
    default_hono = Path("/tmp/grepum-ts-bench/hono.sites.jsonl")
    if "hono" not in tsc and default_hono.exists():
        tsc["hono"] = str(default_hono)

    results = {}
    with tempfile.TemporaryDirectory(prefix="grepum-vs-grep-") as tmp:
        projects = []
        if a.set in ("synthetic", "all"):
            for name, lang in [("shopfront", "TypeScript"), ("datapipe", "Python"), ("arena", "UE5 C++")]:
                projects.append(lambda n=name, l=lang: synthetic(n, l, Path(tmp)))
        if a.set in ("real", "all"):
            for name, r in bench.REAL.items():
                def real(n=name, r=r):
                    p = Project(n, r["lang"], bench.fetch_real(n), None)
                    if n in tsc:
                        p.truth, p.skip_tests = tsc_truth(p, Path(tsc[n])), True
                    return p
                projects.append(real)
        for make in projects:
            p = make()
            print(f"{p.name} ...", flush=True)
            results[p.name] = bench_project(p, a.sample, random.Random(f"{a.seed}-{p.name}"))

    meta = bench.run_meta(argparse.Namespace(graphify="none", reps=1))
    meta.update({"sample": a.sample, "seed": a.seed, "baseline": bench.first_line(["git", "--version"]),
                 "tokens": "bytes / 4", "tsc": sorted(tsc)})
    md = (f"Run {meta['date']} on {meta['machine']}; grepum {meta['grepum']} ({meta['commit']}), {meta['baseline']}; "
          f"{a.sample} sampled questions per project and kind.\n\n" + report(results))
    out = HERE / "results"
    (out / "runs").mkdir(parents=True, exist_ok=True)
    (out / "vs-grep.md").write_text(md)
    (out / "vs-grep.json").write_text(json.dumps(results, indent=1))
    stamp = meta["date"].replace(":", "").replace("-", "")[:15]
    (out / "runs" / f"{stamp}-vs-grep.json").write_text(json.dumps({"meta": meta, "results": results}, indent=1))
    print(md)


if __name__ == "__main__":
    main()
