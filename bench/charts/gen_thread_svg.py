#!/usr/bin/env python3
"""Draw the wasi-threads comparison charts from measured JSON.

Reads every threads-*.json beside this script and writes a matching
threads-*.svg. The JSON is one table: a machine, a thread count, and rows of
benchmark -> host -> milliseconds.

Design: grouped horizontal bars per benchmark, each row scaled to its fastest
entry, with the absolute figure printed at the end of every bar. Rows are
normalised independently because the benchmarks have different durations; the
chart answers "who is faster on this workload", not "which workload is bigger".
Colour is categorical (identity = host) and never cycled. A host with no
measurement is drawn as a gap with the reason in the note, never as a
zero-length bar: a missing number and a fast number must not look alike.
"""

import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent

HOSTS = ("native", "node", "bun", "wnjr")
COLOURS = {"native": "#475569", "node": "#16a34a", "bun": "#7c3aed", "wnjr": "#ea580c"}
LABELS = {"native": "Rust native", "node": "node (V8)", "bun": "bun", "wnjr": "WNJR"}

WIDTH = 980
BAR_HEIGHT = 26
ROW_GAP = 10
LEFT = 210
RIGHT = 130
TOP = 96
BOTTOM = 64


def esc(text):
    return str(text).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def chart(table):
    rows = table["rows"]
    height = TOP + len(rows) * (len(HOSTS) * BAR_HEIGHT + ROW_GAP) + BOTTOM
    plot_width = WIDTH - LEFT - RIGHT
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
        f'viewBox="0 0 {WIDTH} {height}" font-family="ui-sans-serif, system-ui, sans-serif">',
        f'<rect width="{WIDTH}" height="{height}" fill="#ffffff"/>',
        f'<text x="24" y="40" font-size="20" font-weight="600" fill="#0f172a">{esc(table["title"])}</text>',
        f'<text x="24" y="64" font-size="13" fill="#475569">{esc(table["subtitle"])}</text>',
    ]
    y = TOP
    for row in rows:
        values = {host: row["ms"].get(host) for host in HOSTS}
        present = [v for v in values.values() if v]
        best = min(present) if present else 1.0
        out.append(
            f'<text x="24" y="{y + 15}" font-size="14" font-weight="600" fill="#0f172a">{esc(row["name"])}</text>'
        )
        out.append(f'<text x="24" y="{y + 31}" font-size="11" fill="#64748b">{esc(row["note"])}</text>')
        bar_y = y + 4
        for host in HOSTS:
            value = values[host]
            if value is None:
                out.append(
                    f'<text x="{LEFT + 6}" y="{bar_y + 17}" font-size="11" fill="#94a3b8">no measurement</text>'
                )
                bar_y += BAR_HEIGHT
                continue
            length = max(2.0, plot_width * (value / best))
            out.append(
                f'<rect x="{LEFT}" y="{bar_y + 5}" width="{length:.1f}" height="{BAR_HEIGHT - 10}" '
                f'rx="3" fill="{COLOURS[host]}"/>'
            )
            out.append(
                f'<text x="{LEFT + length + 8:.1f}" y="{bar_y + 18}" font-size="12" fill="#0f172a">{value:.1f} ms</text>'
            )
            bar_y += BAR_HEIGHT
        y += len(HOSTS) * BAR_HEIGHT + ROW_GAP
        out.append(
            f'<line x1="24" y1="{y - ROW_GAP // 2}" x2="{WIDTH - 24}" y2="{y - ROW_GAP // 2}" stroke="#e2e8f0" stroke-width="1"/>'
        )
    legend_y = height - 34
    x = 24
    for host in HOSTS:
        out.append(f'<rect x="{x}" y="{legend_y - 10}" width="12" height="12" rx="2" fill="{COLOURS[host]}"/>')
        out.append(f'<text x="{x + 18}" y="{legend_y}" font-size="12" fill="#334155">{esc(LABELS[host])}</text>')
        x += 150
    out.append(
        f'<text x="24" y="{height - 12}" font-size="11" fill="#64748b">'
        f'longer bar = faster on that row; each row scaled to its own best; median of 3 warm runs</text>'
    )
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    files = sorted(HERE.glob("threads-*.json"))
    if not files:
        print("no threads-*.json beside this script", file=sys.stderr)
        return 1
    for path in files:
        table = json.loads(path.read_text(encoding="utf-8"))
        target = path.with_suffix(".svg")
        target.write_text(chart(table), encoding="utf-8")
        print(f"{target.name}: {len(table['rows'])} rows")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
