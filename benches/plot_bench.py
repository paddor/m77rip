#!/usr/bin/env python3
"""Generate benchmark SVGs from cached results.

Results are read from ~/.cache/m77rip/<arch>/ (written by m77rip_bench).

Usage:
    python3 benches/plot_bench.py doc/charts/x86_64
"""

import json
import os
import sys
from pathlib import Path


# Level 0 pipelines. m77rip records encode-only and decode-only benchmark rows,
# so each chart bar is composed from the matching rows below.
CODEC_ORDER = [
    "C++ misa77 -0",
    "C++ misa77 safe -0",
    "m77rip -0",
    "m77rip paranoid -0",
]

COLORS = {
    "C++ misa77 -0":             ("#60a5fa", "#4680c4"),  # blue
    "C++ misa77 safe -0":        ("#93c5fd", "#688eb8"),  # light blue
    "m77rip -0":                 ("#f87171", "#c45050"),  # lz4rip red
    "m77rip paranoid -0":        ("#f472b6", "#c05a92"),  # lz4rip pink
}

LABELS = {
    "C++ misa77 -0":             "C++ misa77 (unsafe decode)",
    "C++ misa77 safe -0":        "C++ misa77 (safe decode)",
    "m77rip -0":                 "m77rip (encapsulated unsafe)",
    "m77rip paranoid -0":        "m77rip (paranoid decode)",
}

PIPELINES = {
    "C++ misa77 -0": (
        "C++ misa77 -0",
        "C++ misa77 -0",
        "C++ misa77 -0",
    ),
    "C++ misa77 safe -0": (
        "C++ misa77 -0",
        "C++ misa77 -0",
        "C++ misa77 safe -0",
    ),
    "m77rip -0": (
        "m77rip compress -0",
        "m77rip compress -0",
        "m77rip (from -0)",
    ),
    "m77rip paranoid -0": (
        "m77rip paranoid compress -0",
        "m77rip paranoid compress -0",
        "m77rip paranoid (from -0)",
    ),
}

ALL_INPUTS = [
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb",
    "reymont", "samba", "sao", "webster", "x-ray", "xml",
]

COMPRESSIBLE = {
    "dickens", "mozilla", "nci", "ooffice", "osdb",
    "reymont", "samba", "webster", "xml",
}
INCOMPRESSIBLE = {"mr", "sao", "x-ray"}


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


def find_result(results, inp, codec):
    return next((r for r in results if r["input"] == inp and r["codec"] == codec), None)


def pipeline_parts(results, inp, codec):
    compress_codec, transfer_codec, decode_codec = PIPELINES[codec]
    comp_r = find_result(results, inp, compress_codec)
    transfer_r = find_result(results, inp, transfer_codec)
    decomp_r = find_result(results, inp, decode_codec)
    if not comp_r or not transfer_r or not decomp_r:
        return None
    if comp_r["compress_ns"] <= 0 or decomp_r["decompress_ns"] <= 0:
        return None
    return comp_r, transfer_r, decomp_r


def pipeline_codecs(results):
    return [
        c for c in CODEC_ORDER
        if any(pipeline_parts(results, inp, c) for inp in ALL_INPUTS)
    ]


def pipeline_stack(results, inp, codec, transfer_rate=1e9):
    parts = pipeline_parts(results, inp, codec)
    if not parts:
        return None
    comp_r, transfer_r, decomp_r = parts
    per_gb = 1e9 / comp_r["input_size"]
    comp = comp_r["compress_ns"] / 1e9 * per_gb
    transfer = (transfer_r["compressed_size"] / comp_r["input_size"]) * (1e9 / transfer_rate)
    decomp = decomp_r["decompress_ns"] / 1e9 * per_gb
    return comp, transfer, decomp


def summary_chart(results, out_path):
    """Aggregate pipeline time per corpus class, lower is better."""
    codecs = pipeline_codecs(results)
    n_codecs = len(codecs)
    hw_label = detect_hardware()
    transfer_rate = 1e9

    groups = [
        ("Compressible", COMPRESSIBLE),
        ("Incompressible", INCOMPRESSIBLE),
    ]

    group_data = {}
    for group_name, file_set in groups:
        for codec in codecs:
            total_input = 0
            total_compressed = 0
            total_compress_ns = 0
            total_decompress_ns = 0
            for inp in ALL_INPUTS:
                if inp not in file_set:
                    continue
                parts = pipeline_parts(results, inp, codec)
                if not parts:
                    continue
                comp_r, transfer_r, decomp_r = parts
                total_input += comp_r["input_size"]
                total_compressed += transfer_r["compressed_size"]
                total_compress_ns += comp_r["compress_ns"]
                total_decompress_ns += decomp_r["decompress_ns"]
            if total_input > 0:
                per_gb = 1e9 / total_input
                comp = total_compress_ns / 1e9 * per_gb
                transfer = (total_compressed / total_input) * (1e9 / transfer_rate)
                decomp = total_decompress_ns / 1e9 * per_gb
                group_data[(group_name, codec)] = (comp, transfer, decomp)

    svg_w = 850
    svg_h = 460
    x_left, x_right = 70, 830
    plot_w = x_right - x_left
    y_top = 55 if hw_label else 45
    y_bot = 310
    plot_h = y_bot - y_top

    y_max = 0
    for v in group_data.values():
        y_max = max(y_max, sum(v))
    y_max *= 1.15

    def y(v):
        return y_bot - (v / y_max) * plot_h

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
        f'misa77 Level 0 Pipeline @1 GB/s: Aggregate across corpus (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

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
            f'{v:.0f}</text>'
        )
        v += step

    # baseline
    L.append(
        f'  <line x1="{x_left}" y1="{y_bot}" x2="{x_right}" y2="{y_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    # y-axis label
    label_y = (y_top + y_bot) / 2
    L.append(
        f'  <text x="22" y="{label_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{label_y})">seconds / GB</text>'
    )

    # bars: 2 groups, n_codecs bars each, with gap between groups
    n_groups = len(groups)
    group_w = plot_w / n_groups
    bar_w = min(group_w * 0.7 / n_codecs, 50)
    inner_gap = bar_w * 0.15
    group_gap = group_w * 0.2

    for gi, (group_name, _) in enumerate(groups):
        group_x = x_left + gi * group_w + group_gap / 2

        for ci, codec in enumerate(codecs):
            if (group_name, codec) not in group_data:
                continue
            comp, transfer, decomp = group_data[(group_name, codec)]
            main_c, xfer_c = COLORS[codec]

            bx = group_x + ci * (bar_w + inner_gap / n_codecs)
            h_comp = (comp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )
            h_transfer = (transfer / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                f' fill="{xfer_c}" rx="1"/>'
            )
            h_decomp = (decomp / y_max) * plot_h
            L.append(
                f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                f' fill="{main_c}" rx="1"/>'
            )

        # group label
        cx = group_x + (n_codecs * (bar_w + inner_gap / n_codecs)) / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{y_bot + 18}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="11" font-weight="600">{group_name}</text>'
        )

    # legend
    leg_y = y_bot + 40
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    row_h = 18
    left_count = (len(legend_items) + 1) // 2
    leg_positions = [(0, r) for r in range(left_count)] + [
        (1, r) for r in range(len(legend_items) - left_count)
    ]
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, _ = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    seg_y = leg_y + left_count * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    L.append('</svg>')

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(L) + "\n")
    print(f"wrote {out_path}")


def pipeline_chart(results, out_path):
    """Per-file pipeline time, one stacked bar per impl per file."""
    codecs = pipeline_codecs(results)
    inputs = [i for i in ALL_INPUTS
              if any(pipeline_parts(results, i, c) for c in codecs)]
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
    svg_h = panel_tops[-1] + panel_h + 120

    # compute stacked values: compress + transfer + decompress (seconds per GB)
    transfer_rate = 1e9
    stacks = {}
    y_max = 0
    for inp in inputs:
        for codec in codecs:
            stack = pipeline_stack(results, inp, codec, transfer_rate)
            if not stack:
                continue
            stacks[(inp, codec)] = stack
            y_max = max(y_max, sum(stack))

    y_max *= 1.1

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
        f'misa77 Level 0: Compress + Transfer @1 GB/s + Decompress (lower is better)'
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
            L.append(
                f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
                f' dominant-baseline="middle" fill="#7d8590" font-size="10">'
                f'{v:.0f}</text>'
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
                f'  <text x="22" y="{total_mid_y}" text-anchor="middle" fill="#e6edf3"'
                f' font-size="11" font-weight="600"'
                f' transform="rotate(-90,22,{total_mid_y})">seconds / GB</text>'
            )

        # bars
        for gi, inp in enumerate(panel_inputs):
            group_x = x_left + gi * group_w + gap / 2

            for ci, codec in enumerate(codecs):
                if (inp, codec) not in stacks:
                    continue
                comp, transfer, decomp = stacks[(inp, codec)]
                main_c, xfer_c = COLORS[codec]

                bx = group_x + ci * bar_w
                h_comp = (comp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
                )
                h_transfer = (transfer / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                    f' fill="{xfer_c}" rx="1"/>'
                )
                h_decomp = (decomp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
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
    row_h = 18
    left_count = (len(legend_items) + 1) // 2
    leg_positions = [(0, r) for r in range(left_count)] + [
        (1, r) for r in range(len(legend_items) - left_count)
    ]
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, _ = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    seg_y = leg_y + left_count * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
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
