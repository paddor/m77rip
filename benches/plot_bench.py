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


ALL_INPUTS = [
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb",
    "reymont", "samba", "sao", "webster", "x-ray", "xml",
]

COMPRESSIBLE = {
    "dickens", "mozilla", "nci", "ooffice", "osdb",
    "reymont", "samba", "webster", "xml",
}
INCOMPRESSIBLE = {"mr", "sao", "x-ray"}

LEVELS = [
    (
        "L-1",
        [
            ("m77rip", "m77rip compress L-1", "m77rip (from L-1)"),
            (
                "m77rip paranoid",
                "m77rip paranoid compress L-1",
                "m77rip paranoid (from L-1)",
            ),
        ],
    ),
    (
        "L0",
        [
            ("C++ misa77", "C++ misa77 -0", "C++ misa77 -0"),
            ("m77rip", "m77rip compress -0", "m77rip (from -0)"),
            (
                "m77rip paranoid",
                "m77rip paranoid compress -0",
                "m77rip paranoid (from -0)",
            ),
        ],
    ),
    (
        "L1",
        [
            ("C++ misa77", "C++ misa77 -1", "C++ misa77 -1"),
            ("m77rip", "m77rip compress -1", "m77rip (from -1)"),
            (
                "m77rip paranoid",
                "m77rip paranoid compress -1",
                "m77rip paranoid (from -1)",
            ),
        ],
    ),
    (
        "L2",
        [
            ("C++ misa77", "C++ misa77 -2", "C++ misa77 -2"),
            ("m77rip", "m77rip compress -2", "m77rip (from -2)"),
            (
                "m77rip paranoid",
                "m77rip paranoid compress -2",
                "m77rip paranoid (from -2)",
            ),
        ],
    ),
]

COLORS = {
    "C++ misa77":       ("#60a5fa", "#4680c4"),
    "m77rip":          ("#f87171", "#c45050"),
    "m77rip paranoid": ("#f472b6", "#c05a92"),
}

LABELS = {
    "C++ misa77":       "C++ misa77",
    "m77rip":          "m77rip",
    "m77rip paranoid": "m77rip paranoid",
}

GROUPS = [
    ("Compressible", COMPRESSIBLE),
    ("Incompressible", INCOMPRESSIBLE),
]

TRANSFER_RATE = 100_000_000
TRANSFER_LABEL = "100 MB/s"


def escape(text):
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def nice_step(max_val, target_lines):
    if max_val <= 0:
        return 1
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
            if line:
                results.append(json.loads(line))
    return results


def base_input(name):
    return name.split("@", 1)[0]


def input_tag(name):
    if "@" not in name:
        return ""
    return name.split("@", 1)[1]


def select_dataset_tag(results):
    tags = {
        input_tag(r["input"])
        for r in results
        if base_input(r["input"]) in ALL_INPUTS
    }
    if "1MiB" in tags:
        return "1MiB"
    if "" in tags:
        return ""
    return sorted(tags)[0] if tags else ""


def input_name(base, tag):
    return f"{base}@{tag}" if tag else base


def dataset_label(tag):
    if tag == "1MiB":
        return "first 1 MiB per Silesia file"
    if tag:
        return f"first {tag} per Silesia file"
    return "full Silesia files"


def find_result(results, inp, codec):
    return next((r for r in results if r["input"] == inp and r["codec"] == codec), None)


def pipeline_parts(results, inp, compress_codec, decode_codec):
    comp_r = find_result(results, inp, compress_codec)
    decomp_r = find_result(results, inp, decode_codec)
    if not comp_r or not decomp_r:
        return None
    if comp_r["compress_ns"] <= 0 or decomp_r["decompress_ns"] <= 0:
        return None
    return comp_r, decomp_r


def aggregate_stack(results, tag, file_set, compress_codec, decode_codec, transfer_rate):
    total_input = 0
    total_compressed = 0
    total_compress_ns = 0.0
    total_decompress_ns = 0.0
    missing = []

    for base in ALL_INPUTS:
        if base not in file_set:
            continue
        inp = input_name(base, tag)
        parts = pipeline_parts(results, inp, compress_codec, decode_codec)
        if not parts:
            missing.append(inp)
            continue
        comp_r, decomp_r = parts
        total_input += comp_r["input_size"]
        total_compressed += comp_r["compressed_size"]
        total_compress_ns += comp_r["compress_ns"]
        total_decompress_ns += decomp_r["decompress_ns"]

    if missing:
        print(
            f"warning: missing {compress_codec} / {decode_codec}: "
            + ", ".join(missing),
            file=sys.stderr,
        )

    if total_input == 0:
        return None
    per_gb = 1e9 / total_input
    compress = total_compress_ns / 1e9 * per_gb
    transfer = (total_compressed / total_input) * (1e9 / transfer_rate)
    decompress = total_decompress_ns / 1e9 * per_gb
    return compress, transfer, decompress


def level_panel_data(results, tag, impls, transfer_rate):
    data = {}
    for group_name, file_set in GROUPS:
        for impl, compress_codec, decode_codec in impls:
            stack = aggregate_stack(
                results, tag, file_set, compress_codec, decode_codec, transfer_rate
            )
            if stack:
                data[(group_name, impl)] = stack
    return data


def draw_grid(L, x_left, x_right, y_top, y_bot, y_max, target_lines):
    def y(v):
        return y_bot - (v / y_max) * (y_bot - y_top)

    step = nice_step(y_max, target_lines)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 7}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="9">{v:.0f}</text>'
        )
        v += step


def draw_panel(L, title, impls, data, x_left, y_top, plot_w, plot_h, grid_lines=4):
    x_right = x_left + plot_w
    y_bot = y_top + plot_h
    y_max = max((sum(v) for v in data.values()), default=1.0) * 1.15
    if y_max <= 0:
        y_max = 1.0

    def y(v):
        return y_bot - (v / y_max) * plot_h

    L.append(
        f'  <text x="{(x_left + x_right) / 2:.1f}" y="{y_top - 18}"'
        f' text-anchor="middle" fill="#e6edf3" font-size="12" font-weight="700">'
        f'{escape(title)}</text>'
    )
    draw_grid(L, x_left, x_right, y_top, y_bot, y_max, grid_lines)
    L.append(
        f'  <line x1="{x_left}" y1="{y_bot}" x2="{x_right}" y2="{y_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )
    mid_y = (y_top + y_bot) / 2
    L.append(
        f'  <text x="{x_left - 43}" y="{mid_y:.1f}" text-anchor="middle"'
        f' fill="#e6edf3" font-size="10" font-weight="600"'
        f' transform="rotate(-90,{x_left - 43},{mid_y:.1f})">seconds / GB</text>'
    )

    group_w = plot_w / len(GROUPS)
    gap = 5
    bar_w = min(group_w * 0.70 / len(impls), 34)
    bars_w = len(impls) * bar_w + (len(impls) - 1) * gap

    for gi, (group_name, _) in enumerate(GROUPS):
        group_left = x_left + gi * group_w
        bars_left = group_left + (group_w - bars_w) / 2
        for ci, (impl, _, _) in enumerate(impls):
            stack = data.get((group_name, impl))
            if not stack:
                continue
            comp, transfer, decomp = stack
            main_c, xfer_c = COLORS[impl]
            bx = bars_left + ci * (bar_w + gap)

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

        group_cx = group_left + group_w / 2
        L.append(
            f'  <text x="{group_cx:.1f}" y="{y_bot + 17}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="10" font-weight="600">'
            f'{group_name}</text>'
        )


def summary_chart(results, out_path):
    tag = select_dataset_tag(results)
    hw_label = detect_hardware()

    panel_data = [
        (level_name, impls, level_panel_data(results, tag, impls, TRANSFER_RATE))
        for level_name, impls in LEVELS
    ]

    svg_w = 980
    x_left = 70
    col_gap = 55
    plot_w = 405
    left_plot_h = 145
    y_top = 78 if hw_label else 64
    row_stride = 215
    right_plot_h = left_plot_h + 2 * row_stride
    left_rows = [y_top + i * row_stride for i in range(3)]
    right_x = x_left + plot_w + col_gap
    plot_bottom = y_top + right_plot_h
    svg_h = plot_bottom + 110
    mid_x = svg_w / 2

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')
    L.append(
        f'  <text x="{mid_x:.1f}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'misa77 Pipelines @{TRANSFER_LABEL} by Level (lower is better)</text>'
    )
    L.append(
        f'  <text x="{mid_x:.1f}" y="39" text-anchor="middle" fill="#7d8590"'
        f' font-size="10">{escape(dataset_label(tag))}</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x:.1f}" y="54" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{escape(hw_label)}</text>'
        )

    for idx, (level_name, impls, data) in enumerate(panel_data[:3]):
        draw_panel(
            L,
            level_name,
            impls,
            data,
            x_left,
            left_rows[idx],
            plot_w,
            left_plot_h,
        )
    level_name, impls, data = panel_data[3]
    draw_panel(L, level_name, impls, data, right_x, y_top, plot_w, right_plot_h, grid_lines=8)

    leg_y = plot_bottom + 52
    legend_items = ["C++ misa77", "m77rip", "m77rip paranoid"]
    leg_start = mid_x - 310
    for i, impl in enumerate(legend_items):
        lx = leg_start + i * 205
        main_c, _ = COLORS[impl]
        L.append(
            f'  <rect x="{lx:.0f}" y="{leg_y - 8}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{leg_y + 2}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{LABELS[impl]}</text>'
        )

    seg_y = leg_y + 26
    L.append(
        f'  <text x="{mid_x - 205:.0f}" y="{seg_y}" fill="#e6edf3"'
        f' font-size="9">bright = encode + decode</text>'
    )
    L.append(
        f'  <text x="{mid_x + 25:.0f}" y="{seg_y}" fill="#7d8590"'
        f' font-size="9">dim = transfer @{TRANSFER_LABEL}</text>'
    )

    L.append("</svg>")

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


if __name__ == "__main__":
    main()
