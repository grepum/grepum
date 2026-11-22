#!/usr/bin/env python3
"""Render the README benchmark charts from benchmark/results/{synthetic,real,vs-grep}.json.

    python3 benchmark/charts.py      # writes benchmark/charts/*-{light,dark}.svg

Static SVG (GitHub READMEs show images, so there is no hover layer); every bar
carries its value at the tip, and the full numbers are in results/<set>.md.
"""
import json
import pathlib
from html import escape

ROOT = pathlib.Path(__file__).resolve().parent.parent
RESULTS = ROOT / "benchmark" / "results"
OUT = ROOT / "benchmark" / "charts"

SYNTHETIC = [("shopfront", "TypeScript"), ("datapipe", "Python"), ("arena", "UE5 C++")]
REAL = [(r["name"], r["lang"]) for r in json.loads((ROOT / "benchmark" / "real_repos.json").read_text())["repos"]]
TOOLS = [("grepum", "grepum"), ("graphify", "graphify")]

THEMES = {
    "light": dict(surface="#fcfcfb", ink="#0b0b0b", ink2="#52514e", muted="#898781",
                  baseline="#c3c2b7", series=["#2a78d6", "#eb6834"]),
    "dark": dict(surface="#1a1a19", ink="#f0efec", ink2="#c3c2b7", muted="#898781",
                 baseline="#383835", series=["#3987e5", "#d95926"]),
}
FONT = 'system-ui, -apple-system, "Segoe UI", Helvetica, Arial, sans-serif'
BAR = 10        # bar thickness
PAIR_GAP = 2    # surface gap between a project's two bars
ROW = 44        # height of one project row
RADIUS = 4      # rounded data-end


def fmt_time(s):
    return f"{s * 1000:.0f} ms" if s < 1 else f"{s:.2f} s"


def fmt_bytes(b):
    kib = b / 1024
    return f"{kib:.0f} KB" if kib < 1024 else f"{kib / 1024:.1f} MB"


def fmt_ratio(x):
    return f"{x:.0f}×" if x >= 10 else f"{x:.1f}×"


def bar(x, y, w, color):
    """Horizontal bar: square at the baseline, 4px rounded at the data end."""
    w = max(w, 2.0)
    r = min(RADIUS, w / 2, BAR / 2)
    return (f'<path d="M{x:.1f},{y:.1f} h{w - r:.1f} a{r},{r} 0 0 1 {r},{r} v{BAR - 2 * r:.1f} '
            f'a{r},{r} 0 0 1 {-r},{r} h{-(w - r):.1f} z" fill="{color}"/>')


class Svg:
    def __init__(self, width, theme):
        self.w, self.t, self.parts, self.h = width, THEMES[theme], [], 0

    def text(self, x, y, s, size=13, fill="ink", weight=400, anchor="start"):
        color = self.t.get(fill, fill)
        self.parts.append(f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" font-weight="{weight}" '
                          f'fill="{color}" text-anchor="{anchor}">{escape(s)}</text>')

    def raw(self, s):
        self.parts.append(s)

    def render(self, title):
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.w}" height="{self.h}" '
                f'viewBox="0 0 {self.w} {self.h}" font-family=\'{FONT}\' role="img">'
                f'<title>{escape(title)}</title>'
                f'<rect width="100%" height="100%" rx="8" fill="{self.t["surface"]}"/>'
                + "".join(self.parts) + "</svg>\n")


def fmt_tokens(n):
    return f"{n:.0f}" if n < 1000 else f"{n / 1000:.1f}k"


def header(svg, title, subtitle, tools=TOOLS):
    svg.text(24, 34, title, size=17, weight=600)
    svg.text(24, 56, subtitle, size=13, fill="ink2")
    # legend: swatch + name, text in ink (never the series color)
    x = 24
    for i, (_, name) in enumerate(tools):
        svg.raw(f'<rect x="{x}" y="72" width="12" height="12" rx="3" fill="{svg.t["series"][i]}"/>')
        svg.text(x + 18, 82.5, name, size=13)
        x += 18 + 8 * len(name) + 28
    return 104


def panel(svg, x, y, width, title, projects, values, fmt, note, caption=None):
    """One small multiple: projects × (grepum, other tool) horizontal bars on a shared scale.

    values[p] = (grepum, other); note(p) -> annotation after grepum's value, or None.
    caption: a muted line under the title saying what each bar measures.
    """
    svg.text(x, y + 14, title, size=14, weight=600)
    if caption:
        svg.text(x, y + 31, caption, size=12, fill="muted")
        y += 17
    label_w = 92
    x0 = x + label_w
    # leave room after the longest bar for its value label
    span = width - label_w - 70
    vmax = max(v for pair in values.values() for v in pair)
    top = y + 30
    for i, (proj, lang) in enumerate(projects):
        ry = top + i * ROW
        svg.text(x, ry + 11, proj, size=13)
        svg.text(x, ry + 26, lang, size=11, fill="muted")
        for j, v in enumerate(values[proj]):
            by = ry + 2 + j * (BAR + PAIR_GAP)
            w = span * v / vmax if vmax else 0
            svg.raw(bar(x0, by, w, svg.t["series"][j]))
            label = fmt(v)
            extra = note(proj) if j == 0 else None
            tail = (f'<tspan dx="10" font-weight="600" fill="{svg.t["ink"]}">{escape(extra)}</tspan>'
                    if extra else "")
            svg.raw(f'<text x="{x0 + max(w, 2) + 6:.1f}" y="{by + 9.5:.1f}" font-size="12" '
                    f'fill="{svg.t["ink2"]}">{escape(label)}{tail}</text>')
    bottom = top + len(projects) * ROW - 14
    svg.raw(f'<line x1="{x0}" y1="{top - 4}" x2="{x0}" y2="{bottom}" stroke="{svg.t["baseline"]}" stroke-width="1"/>')
    return bottom + 10


def speed_chart(data, theme, projects, title, subtitle):
    W = 840
    svg = Svg(W, theme)
    y = header(svg, title, subtitle)
    col_w = (W - 48 - 32) / 2
    panels = [
        ("Cold index", "cold", fmt_time, "faster"),
        ("Re-index after editing one file", "edit", fmt_time, "faster"),
        ("Index 4 new worktrees", "worktrees_total", fmt_time, "faster"),
        ("Disk used (main + 4 worktrees)", "disk", fmt_bytes, "less"),
    ]
    row_bottom = y
    for k, (title, key, fmt, word) in enumerate(panels):
        px = 24 + (k % 2) * (col_w + 32)
        if k % 2 == 0:
            y = row_bottom + (16 if k else 0)
        values = {p: (data[p]["grepum"][key], data[p]["graphify"][key]) for p, _ in projects}
        b = panel(svg, px, y, col_w, title, projects, values, fmt,
                  lambda p, v=values, w=word: f"{fmt_ratio(v[p][1] / v[p][0])} {w}")
        row_bottom = max(row_bottom, b)
    svg.h = int(row_bottom + 14)
    return svg.render(title)


def accuracy_chart(data, theme, projects=SYNTHETIC):
    W = 840
    svg = Svg(W, theme)
    y = header(svg, "Synthetic projects: call-graph accuracy",
               "Call edges checked against the call graph recorded by the project generator. Longer is better.")
    col_w = (W - 48 - 32) / 2
    bottom = y
    for k, (title, key) in enumerate([("Recall: real calls found", "recall"),
                                       ("Precision: reported calls that are real", "precision")]):
        px = 24 + k * (col_w + 32)
        values = {p: tuple(data[p][t]["accuracy"][key] for t, _ in TOOLS) for p, _ in projects}
        b = panel(svg, px, y, col_w, title, projects, values, lambda v: f"{v * 100:.1f}%", lambda p: None)
        bottom = max(bottom, b)
    svg.h = int(bottom + 14)
    return svg.render("grepum vs graphify: call-edge recall and precision")


GREP_TOOLS = [("grepum", "grepum"), ("grep", "grep and reading files, without grepum")]


def vs_grep_tokens_chart(data, theme, projects):
    W = 840
    svg = Svg(W, theme)
    y = header(svg, "Tokens an agent reads to answer: grepum vs grep",
               "Median per question, 50 sampled questions per repo. Tokens ≈ bytes / 4. Shorter is better.", GREP_TOOLS)
    col_w = (W - 48 - 32) / 2
    panels = [
        ("Who calls X?", "callers", "tokens", "grepum callers  vs  git grep -n -w X"),
        ("Who calls X, with their code", "callers", "code_tokens", "callers | show -  vs  git grep, then read each file hit"),
        ("Read one function", "show", "tokens", "grepum show  vs  git grep, then read its file"),
        ("What's in this file?", "outline", "tokens", "grepum outline  vs  reading the file"),
    ]
    row_bottom = y
    for k, (title, q, key, caption) in enumerate(panels):
        px = 24 + (k % 2) * (col_w + 32)
        if k % 2 == 0:
            y = row_bottom + (16 if k else 0)
        values = {p: (data[p][q]["grepum"][key], data[p][q]["grep"][key]) for p, _ in projects}
        b = panel(svg, px, y, col_w, title, projects, values, fmt_tokens,
                  lambda p, v=values: f"{fmt_ratio(v[p][1] / v[p][0])} fewer" if v[p][0] else None, caption)
        row_bottom = max(row_bottom, b)
    svg.h = int(row_bottom + 14)
    return svg.render("grepum vs grep: tokens read per question")


def vs_grep_accuracy_chart(data, theme, projects):
    W = 840
    svg = Svg(W, theme)
    tools = [("grepum", "grepum callers"), ("grep", "grep (function around each match)")]
    y = header(svg, "Who calls X? Answers checked against the true callers",
               "Synthetic projects: the generator's call graph. hono: the TypeScript compiler. Longer is better.", tools)
    col_w = (W - 48 - 32) / 2
    bottom = y
    for k, (title, key) in enumerate([("Recall: true callers found", "recall"), ("Precision: answers that are true callers", "precision")]):
        px = 24 + k * (col_w + 32)
        values = {p: tuple(data[p]["callers"]["accuracy"][t][key] for t, _ in tools) for p, _ in projects}
        b = panel(svg, px, y, col_w, title, projects, values, lambda v: f"{v * 100:.1f}%", lambda p: None)
        bottom = max(bottom, b)
    svg.h = int(bottom + 14)
    return svg.render("grepum vs grep: who-calls recall and precision")


def latest_meta(set_name):
    runs = sorted((RESULTS / "runs").glob(f"*-{set_name}.json"))
    return json.loads(runs[-1].read_text())["meta"] if runs else {}


def machine(meta):
    return meta.get("machine", "").split(",")[0] or "this machine"


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    written = 0
    syn_path, real_path = RESULTS / "synthetic.json", RESULTS / "real.json"
    if syn_path.exists():
        data, meta = json.loads(syn_path.read_text()), latest_meta("synthetic")
        sub = (f"Generated ~200-file projects, median of {meta.get('reps', 3)} runs on {machine(meta)}. "
               "Shorter is better.")
        for theme in THEMES:
            (OUT / f"synthetic-speed-{theme}.svg").write_text(
                speed_chart(data, theme, SYNTHETIC, "Synthetic projects: speed and disk", sub))
            (OUT / f"synthetic-accuracy-{theme}.svg").write_text(accuracy_chart(data, theme))
            written += 2
    if real_path.exists():
        data, meta = json.loads(real_path.read_text()), latest_meta("real")
        projects = [p for p in REAL if p[0] in data]
        sub = (f"Open-source repos at pinned tags, median of {meta.get('reps', 3)} runs on {machine(meta)}. "
               "Shorter is better.")
        for theme in THEMES:
            (OUT / f"real-speed-{theme}.svg").write_text(
                speed_chart(data, theme, projects, "Real repositories: speed and disk", sub))
            written += 1
    vs_path = RESULTS / "vs-grep.json"
    if vs_path.exists():
        data = json.loads(vs_path.read_text())
        real = [p for p in REAL if p[0] in data]
        scored = [p for p in SYNTHETIC + [("hono", "TypeScript")] if p[0] in data and "accuracy" in data[p[0]]["callers"]]
        scored = [(n, "TS, vs tsc" if n == "hono" else lang) for n, lang in scored]
        for theme in THEMES:
            (OUT / f"vs-grep-tokens-{theme}.svg").write_text(vs_grep_tokens_chart(data, theme, real))
            (OUT / f"vs-grep-accuracy-{theme}.svg").write_text(vs_grep_accuracy_chart(data, theme, scored))
            written += 2
    print(f"wrote {written} charts to {OUT.relative_to(ROOT)}/")


if __name__ == "__main__":
    main()
