#!/usr/bin/env python3
"""Compare grepum call edges (grepum export) with tsc-resolved call sites.

usage: compare.py <grepum-export.json> <sites.jsonl> [--dump DIR]
Unit = (caller file, call-site line, callee def). Callers in test files are dropped on both sides.
"""
import collections, json, re, sys, os

g = json.load(open(sys.argv[1]))
sites = [json.loads(l) for l in open(sys.argv[2])]
dump = sys.argv[sys.argv.index("--dump") + 1] if "--dump" in sys.argv else None
TEST = re.compile(r"(\.test\.|\.spec\.|__tests__|/tests?/|/mocks?/|/fixtures?/|benchmarks?/|\.d\.ts$)")

defs = g["defs"]
by_file = collections.defaultdict(list)
for d in defs:
    by_file[d["file"]].append(d)

def gdef_for(decl):
    """Map a tsc declaration to a grepum def id (same file, same name, containing line)."""
    best = None
    for d in by_file.get(decl["file"], []):
        if d["name"] != decl["name"]:
            continue
        if d["line"] <= decl["line"] <= d["end_line"]:
            if best is None or d["end_line"] - d["line"] < best["end_line"] - best["line"]:
                best = d
    if best is None:  # a def of that name starting nearby (decorators, export default, overloads)
        near = [d for d in by_file.get(decl["file"], []) if d["name"] == decl["name"] and abs(d["line"] - decl["line"]) <= 3]
        best = near[0] if near else None
    if best is None:
        return None
    # Overloads are one function: grepum links the first signature, tsc the matching one.
    same = [d for d in by_file.get(decl["file"], []) if d["qual"] == best["qual"]]
    return min(same, key=lambda d: d["line"])["id"]

# grepum edges
gedges = []
for e in g["edges"]:
    if e["kind"] != "call" or TEST.search(e["from_file"]):
        continue
    if TEST.search(defs[e["to"]]["file"]):
        continue
    gedges.append(e)

# index tsc sites by file
site_by_file = collections.defaultdict(list)
truth = []          # (site, gid)
missing_def = []    # tsc eligible target that grepum never extracted as a def
for s in sites:
    site_by_file[s["file"]].append(s)
    if s["status"] == "project" and s["decl"]["eligible"] and not TEST.search(s["decl"]["file"]):
        gid = gdef_for(s["decl"])
        s["gid"] = gid
        if gid is None:
            missing_def.append(s)
        else:
            truth.append((s, gid))

def covers(s, ln):
    return s["line"] <= ln <= max(s["end"], s["nline"]) and (ln == s["line"] or ln == s["nline"])

# grepum keeps one edge per (caller, callee): compare at that level.
CALLER_KINDS = {"fn", "method", "class"}
def caller_of(file, ln):
    best = None
    for d in by_file.get(file, []):
        if d["kind"] in CALLER_KINDS and d["line"] <= ln <= d["end_line"]:
            if best is None or d["end_line"] - d["line"] <= best["end_line"] - best["line"]:
                best = d
    return best["id"] if best else None

tpairs = {}
for s, gid in truth:
    c = caller_of(s["file"], s["line"])
    if c is not None:
        tpairs.setdefault((c, gid), s)
gpairs = {}
for e in gedges:
    if e["from"] is not None:
        gpairs.setdefault((e["from"], e["to"]), e)
hit = [s for k, s in tpairs.items() if k in gpairs]
miss = [s for k, s in tpairs.items() if k not in gpairs]
truth = [(s, k[1]) for k, s in tpairs.items()]
fp, unjudged = [], []
for k, e in gpairs.items():
    if k in tpairs:
        continue
    cdef, tgt = defs[k[0]], defs[k[1]]
    cands = [s for s in site_by_file.get(cdef["file"], []) if cdef["line"] <= s["line"] <= cdef["end_line"] and caller_of(cdef["file"], s["line"]) == k[0]]
    same_name = [s for s in cands if re.split(r"[.<(]", s["text"].replace("?.", "."))[-1].strip() == tgt["name"] or s["text"].endswith(tgt["name"])]
    if not same_name:
        unjudged.append((e, "no tsc site with that name in caller"))
        continue
    st = {x["status"] for x in same_name}
    s = same_name[0]
    if any(x["status"] == "project" and x.get("gid") == k[1] for x in same_name):
        unjudged.append((e, "tsc agrees but caller attribution differs"))
    elif st == {"unresolved"}:
        unjudged.append((e, "tsc unresolved"))
    elif "project" in st:
        s = next(x for x in same_name if x["status"] == "project")
        fp.append((e, s, "tsc: different project def" if s["decl"]["eligible"] else "tsc: non-function project symbol (" + s["decl"]["kind"] + ")"))
    else:
        fp.append((e, s, "tsc: external/lib"))
gedges = list(gpairs.values())

P = len(gedges) - len(unjudged)
prec = (P - len(fp)) / P if P else 0
rec = len(hit) / len(truth) if truth else 0
print(f"tsc sites={len(sites)} truth edges={len(truth)} (+{len(missing_def)} to targets grepum didn't extract)")
print(f"grepum call edges (non-test)={len(gedges)} judged={P} unjudged={len(unjudged)}")
print(f"recall={rec:.3f} ({len(hit)}/{len(truth)})  recall incl. unextracted targets={len(hit)/(len(truth)+len(missing_def)):.3f}")
print(f"precision={prec:.3f} ({P-len(fp)}/{P})")

def feat(s):
    d = s["decl"]
    tags = [s["kind"]]
    if s["kind"] == "call":
        tags.append("recv=" + s["recv"])
    if s["optional"]:
        tags.append("optional")
    if s["hops"] > 1:
        tags.append("reexport")
    if s.get("imp"):
        spec = s["imp"]["spec"]
        tags.append("import:" + ("rel.js" if spec.startswith(".") and spec.endswith(".js") else "rel" if spec.startswith(".") else "nonrel"))
        tags.append("form:" + s["imp"]["form"])
    tags.append("decl=" + d["kind"] + ("(abstract)" if d.get("abstract") else "") + ("@" + d["containerKind"] if d["kind"] in ("MethodSignature", "MethodDeclaration", "PropertyDeclaration", "PropertyAssignment") else ""))
    return tags

def table(rows, title, n=18):
    print(f"\n{title}")
    c = collections.Counter()
    for s in rows:
        for t in feat(s):
            c[t] += 1
    tot = collections.Counter()
    for s in truth_all:
        for t in feat(s):
            tot[t] += 1
    for t, k in c.most_common(n):
        print(f"  {k:5d} / {tot[t]:5d}  {t}")

truth_all = [s for s, _ in truth]
table(miss, "MISSES by feature (missed / truth total)")
print(f"\nunextracted target kinds: {collections.Counter(s['decl']['kind'] + '@' + str(s['decl']['containerKind']) for s in missing_def).most_common(8)}")
print(f"\nFALSE edges by tsc verdict: {collections.Counter(r for _, _, r in fp).most_common()}")
print(f"unjudged: {collections.Counter(r for _, r in unjudged).most_common()}")

if dump:
    os.makedirs(dump, exist_ok=True)
    with open(f"{dump}/miss.jsonl", "w") as f:
        for s in miss:
            f.write(json.dumps({"feat": feat(s), **s}) + "\n")
    with open(f"{dump}/fp.jsonl", "w") as f:
        for e, s, r in fp:
            f.write(json.dumps({"why": r, "grepum_to": defs[e["to"]]["qual"], "to_file": defs[e["to"]]["file"], "from": defs[e["from"]]["qual"] if e["from"] is not None else None, "site": s}) + "\n")
    with open(f"{dump}/missing_def.jsonl", "w") as f:
        for s in missing_def:
            f.write(json.dumps(s) + "\n")
    with open(f"{dump}/unjudged.jsonl", "w") as f:
        for e, r in unjudged:
            f.write(json.dumps({"why": r, "to": defs[e["to"]]["qual"], "file": e["from_file"], "line": e["line"]}) + "\n")
