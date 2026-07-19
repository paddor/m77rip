#!/usr/bin/env python3
"""Generate decompression benchmark SVGs from cached results.

Results are read from ~/.cache/m77rip/<arch>/ (written by m77rip_bench).

Usage:
    python3 benches/plot_bench.py doc/charts/x86_64
"""

import json
import os
import sys
from pathlib import Path


# The 4 implementations we chart (level 0 only).
CODEC_ORDER = [
    "C++ misa77 -0",
    "C++ misa77 safe -0",
    "m77rip (from -0)",
    "m77rip paranoid (from -0)",
]

COLORS = {
    "C++ misa77 -0":              "#60a5fa",   # blue
    "C++ misa77 safe -0":         "#93c5fd",   # light blue
    "m77rip (from -0)":           "#f87171",   # lz4rip red
    "m77rip paranoid (from -0)":  "#f472b6",   # lz4rip purple
}

LABELS = {
    "C++ misa77 -0":              "C++ misa77 (unsafe)",
    "C++ misa77 safe -0":         "C++ misa77 (safe)",
    "m77rip (from -0)":           "m77rip (encapsulated unsafe)",
    "m77rip paranoid (from -0)":  "m77rip (paranoid)",
}

ALL_INPUTS = [
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb",
    "reymont", "samba", "sao", "webster", "x-ray", "xml",
]


def human_size(n):
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f} MB"
    if n >= 1_000:
        return f"{n / 1_000:.0f} KB"
    return f"{n} B"


def nice_step(max_val, target_lines):
    raw = max_val / target_lines
    mag = 10 ** int(f"{raw:.0e}".split("e")[1])
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= target_lines + 1:
            return step
    return mag * 10


def detect_hardware():
    try:
        cpu = os.environ.get("M77RIP_CPU")
        if not cpu:
            for line in open("/proc/cpuinfo"):
                if line.startswith("model name"):
                    cpu = line.split(":", 1)[1].strip()
                    cpu = cpu.replace("(R)", "").replace("(TM)", "").replace("CPU ", "")
                    break
        if cpu:
            label = cpu
            extras = []
            try:
                gov = open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read().strip()
                if gov == "performance":
                    extras.append("performance governor")
            except OSError:
                pass
            for path, off_val in [
                ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
                ("/sys/devices/system/cpu/cpufreq/boost", "0"),
            ]:
                try:
                    if open(path).read().strip() == off_val:
                        extras.append("turbo off")
                    break
                except OSError:
                    continue
            hw_extras = os.environ.get("M77RIP_HW_EXTRAS")
            if hw_extras:
                extras.extend(hw_extras.split(","))
            if extras:
                label += ", " + ", ".join(extras)
            return label
    except OSError:
        pass
    return None


def load_results():
    arch = os.uname().machine
    cache_dir = Path.home() / ".cache" / "m77rip" / arch
    results = []
    for path in sorted(cache_dir.glob("*.jsonl")):
        for line in path.read_text().splitlines():
            line = line.strip()
            if not line:
                continue
            results.append(json.loads(line))
    return results


def geomean(values):
    if not values:
        return 0.0
    product = 1.0
    for v in values:
        product *= v
    return product ** (1.0 / len(values))


def summary_chart(results, out_path):
    """One bar per implementation, aggregated (geometric mean MB/s) across files."""
    codecs = [c for c in CODEC_ORDER if any(r["codec"] == c for r in results)]

    hw_label = detect_hardware()

    # Compute geometric mean MB/s per codec
    codec_mbps = {}
    for codec in codecs:
        values = []
        for inp in ALL_INPUTS:
            r = next((r for r in results
                      if r["input"] == inp and r["codec"] == codec
                      and r["decompress_ns"] > 0), None)
            if r:
                values.append(r["input_size"] / r["decompress_ns"] * 1000.0)
        codec_mbps[codec] = geomean(values)

    y_max = max(codec_mbps.values()) * 1.15

    svg_w = 850
    svg_h = 340
    x_left, x_right = 70, 830
    plot_w = x_right - x_left
    top_margin = 50 if hw_label else 40
    p_top = top_margin
    p_bot = svg_h - 80

    mid_x = (x_left + x_right) / 2
    n_codecs = len(codecs)
    bar_w = min(90, plot_w / n_codecs * 0.65)
    total_bars_w = n_codecs * bar_w + (n_codecs - 1) * bar_w * 0.4
    bar_start = mid_x - total_bars_w / 2

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'Decompression Throughput (Silesia geomean)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    def y(v):
        return p_bot - (v / y_max) * (p_bot - p_top)

    # y gridlines
    step = nice_step(y_max, 5)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="10">'
            f'{int(v)}</text>'
        )
        v += step

    # baseline
    L.append(
        f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    # y-axis label
    label_y = (p_top + p_bot) / 2
    L.append(
        f'  <text x="18" y="{label_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,18,{label_y})">MB/s</text>'
    )

    # bars
    for ci, codec in enumerate(codecs):
        val = codec_mbps[codec]
        color = COLORS[codec]
        bx = bar_start + ci * (bar_w * 1.4)
        h = (val / y_max) * (p_bot - p_top)
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(val):.1f}"'
            f' width="{bar_w:.1f}" height="{h:.1f}"'
            f' fill="{color}" rx="2"/>'
        )
        # value label on top of bar
        L.append(
            f'  <text x="{bx + bar_w / 2:.1f}" y="{y(val) - 6:.1f}"'
            f' text-anchor="middle" fill="#e6edf3" font-size="10"'
            f' font-weight="600">{int(val)}</text>'
        )

    # legend
    leg_y = svg_h - 25
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    col_w = 200
    total_leg_w = len(legend_items) * col_w
    leg_start_x = mid_x - total_leg_w / 2
    for i, (key, label) in enumerate(legend_items):
        color = COLORS[key]
        lx = leg_start_x + i * col_w
        ly = leg_y
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 9}" width="12" height="12"'
            f' fill="{color}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    L.append('</svg>')

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(L) + "\n")
    print(f"wrote {out_path}")


def pipeline_chart(results, out_path):
    """Per-file decompression throughput, one bar per impl per file, two panels."""
    codecs = [c for c in CODEC_ORDER if any(r["codec"] == c for r in results)]
    inputs = [i for i in ALL_INPUTS
              if any(r["input"] == i and r["codec"] in set(codecs) for r in results)]
    n_codecs = len(codecs)

    mid = (len(inputs) + 1) // 2
    panels = [inputs[:mid], inputs[mid:]]

    hw_label = detect_hardware()

    svg_w = 850
    x_left, x_right = 55, 830
    plot_w = x_right - x_left
    panel_h = 240
    panel_gap = 70
    top_margin = 50 if hw_label else 40

    panel_tops = [top_margin, top_margin + panel_h + panel_gap]
    svg_h = panel_tops[-1] + panel_h + 100

    # compute MB/s
    mbps = {}
    y_max = 0
    for inp in inputs:
        for codec in codecs:
            r = next((r for r in results
                      if r["input"] == inp and r["codec"] == codec
                      and r["decompress_ns"] > 0), None)
            if not r:
                continue
            val = r["input_size"] / r["decompress_ns"] * 1000.0
            mbps[(inp, codec)] = val
            y_max = max(y_max, val)

    y_max *= 1.12

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'Decompression: Per-File Throughput (higher is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    for pi, panel_inputs in enumerate(panels):
        n_inputs = len(panel_inputs)
        p_top = panel_tops[pi]
        p_bot = p_top + panel_h

        group_w = plot_w / n_inputs
        bar_w = group_w * 0.75 / n_codecs
        gap = group_w * 0.25

        def y(v, _bot=p_bot, _top=p_top):
            return _bot - (v / y_max) * (_bot - _top)

        # y gridlines
        step = nice_step(y_max, 5)
        v = step
        while v <= y_max:
            yy = y(v)
            L.append(
                f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
                f' stroke="#21262d" stroke-width="1"/>'
            )
            if pi == 0:
                L.append(
                    f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
                    f' dominant-baseline="middle" fill="#7d8590" font-size="10">'
                    f'{int(v)}</text>'
                )
            v += step

        # baseline
        L.append(
            f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
            f' stroke="#30363d" stroke-width="1.5"/>'
        )

        # y-axis label (first panel only)
        if pi == 0:
            total_mid_y = (panel_tops[0] + panel_tops[1] + panel_h) / 2
            L.append(
                f'  <text x="18" y="{total_mid_y}" text-anchor="middle" fill="#e6edf3"'
                f' font-size="11" font-weight="600"'
                f' transform="rotate(-90,18,{total_mid_y})">MB/s</text>'
            )

        # bars
        for gi, inp in enumerate(panel_inputs):
            group_x = x_left + gi * group_w + gap / 2

            for ci, codec in enumerate(codecs):
                if (inp, codec) not in mbps:
                    continue
                val = mbps[(inp, codec)]
                color = COLORS[codec]

                bx = group_x + ci * bar_w
                h = (val / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(val):.1f}"'
                    f' width="{bar_w:.1f}" height="{h:.1f}"'
                    f' fill="{color}" rx="1"/>'
                )

            # group label
            r0 = next((r for r in results if r["input"] == inp), None)
            size_label = human_size(r0["input_size"]) if r0 else ""
            cx = group_x + (n_codecs * bar_w) / 2
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 16}" text-anchor="middle"'
                f' fill="#e6edf3" font-size="10" font-weight="600">{inp}</text>'
            )
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 28}" text-anchor="middle"'
                f' fill="#7d8590" font-size="9">{size_label}</text>'
            )

    # legend
    leg_y = panel_tops[-1] + panel_h + 50
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    col_w = 200
    total_leg_w = len(legend_items) * col_w
    leg_start_x = mid_x - total_leg_w / 2
    for i, (key, label) in enumerate(legend_items):
        color = COLORS[key]
        lx = leg_start_x + i * col_w
        ly = leg_y
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 9}" width="12" height="12"'
            f' fill="{color}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    L.append('</svg>')

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(L) + "\n")
    print(f"wrote {out_path}")


def main():
    if len(sys.argv) < 2:
        print(f"usage: {sys.argv[0]} <output-dir>", file=sys.stderr)
        sys.exit(1)

    out_dir = Path(sys.argv[1])
    results = load_results()
    if not results:
        print("no results found in ~/.cache/m77rip/", file=sys.stderr)
        sys.exit(1)

    summary_chart(results, out_dir / "summary.svg")
    pipeline_chart(results, out_dir / "pipeline.svg")


if __name__ == "__main__":
    main()
