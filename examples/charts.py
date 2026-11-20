#!/usr/bin/env python3
"""Render the README benchmark charts from examples/results/latest.json.

    python3 examples/charts.py            # writes docs/bench-*-{light,dark}.svg

Static SVG (GitHub READMEs show images, so there is no hover layer); every bar
carries its value at the tip, and the full numbers are in results/latest.md.
"""
import json
import pathlib
from html import escape

ROOT = pathlib.Path(__file__).resolve().parent.parent
RESULTS = ROOT / "examples" / "results" / "latest.json"
OUT = ROOT / "docs"

PROJECTS = [("shopfront", "TypeScript"), ("datapipe", "Python"), ("arena", "Unreal Engine 5 C++")]
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


def header(svg, title, subtitle):
    svg.text(24, 34, title, size=17, weight=600)
    svg.text(24, 56, subtitle, size=13, fill="ink2")
    # legend: swatch + name, text in ink (never the series color)
    x = 24
    for i, (_, name) in enumerate(TOOLS):
        svg.raw(f'<rect x="{x}" y="72" width="12" height="12" rx="3" fill="{svg.t["series"][i]}"/>')
        svg.text(x + 18, 82.5, name, size=13)
        x += 18 + 8 * len(name) + 28
    return 104


def panel(svg, x, y, width, title, values, fmt, note):
    """One small multiple: 3 projects × (grepum, graphify) horizontal bars on a shared scale.

    values[p] = (grepum, graphify); note(p) -> annotation after grepum's value, or None.
    """
    svg.text(x, y + 14, title, size=14, weight=600)
    label_w = 92
    x0 = x + label_w
    # leave room after the longest bar for its value label
    span = width - label_w - 70
    vmax = max(v for pair in values.values() for v in pair)
    top = y + 30
    for i, (proj, lang) in enumerate(PROJECTS):
        ry = top + i * ROW
        svg.text(x, ry + 11, proj, size=13)
        svg.text(x, ry + 26, lang if len(lang) < 14 else "UE5 C++", size=11, fill="muted")
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
    bottom = top + len(PROJECTS) * ROW - 14
    svg.raw(f'<line x1="{x0}" y1="{top - 4}" x2="{x0}" y2="{bottom}" stroke="{svg.t["baseline"]}" stroke-width="1"/>')
    return bottom + 10


def speed_chart(data, theme):
    W = 840
    svg = Svg(W, theme)
    y = header(svg, "grepum vs graphify: speed and disk",
               "Generated ~200-file projects, median of repeated runs on an Apple M2 Max. Shorter is better.")
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
        values = {p: (data[p]["grepum"][key], data[p]["graphify"][key]) for p, _ in PROJECTS}
        b = panel(svg, px, y, col_w, title, values, fmt,
                  lambda p, v=values, w=word: f"{fmt_ratio(v[p][1] / v[p][0])} {w}")
        row_bottom = max(row_bottom, b)
    svg.h = int(row_bottom + 14)
    return svg.render("grepum vs graphify: indexing time and disk use")


def accuracy_chart(data, theme):
    W = 840
    svg = Svg(W, theme)
    y = header(svg, "grepum vs graphify: call-graph accuracy",
               "Call edges checked against the call graph recorded by the project generator. Longer is better.")
    col_w = (W - 48 - 32) / 2
    bottom = y
    for k, (title, key) in enumerate([("Recall: real calls found", "recall"),
                                       ("Precision: reported calls that are real", "precision")]):
        px = 24 + k * (col_w + 32)
        values = {p: tuple(data[p][t]["accuracy"][key] for t, _ in TOOLS) for p, _ in PROJECTS}
        b = panel(svg, px, y, col_w, title, values, lambda v: f"{v * 100:.1f}%", lambda p: None)
        bottom = max(bottom, b)
    svg.h = int(bottom + 14)
    return svg.render("grepum vs graphify: call-edge recall and precision")


def main():
    data = json.loads(RESULTS.read_text())
    OUT.mkdir(exist_ok=True)
    for theme in THEMES:
        (OUT / f"bench-speed-{theme}.svg").write_text(speed_chart(data, theme))
        (OUT / f"bench-accuracy-{theme}.svg").write_text(accuracy_chart(data, theme))
    print(f"wrote {len(THEMES) * 2} charts to {OUT.relative_to(ROOT)}/")


if __name__ == "__main__":
    main()
