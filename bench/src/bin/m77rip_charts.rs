#![allow(clippy::too_many_arguments)]

use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

const BG: RGBColor = RGBColor(0x0d, 0x11, 0x17);
const GRID: RGBColor = RGBColor(0x21, 0x26, 0x2d);
const AXIS: RGBColor = RGBColor(0x30, 0x36, 0x3d);
const TEXT: RGBColor = RGBColor(0xe6, 0xed, 0xf3);
const MUTED: RGBColor = RGBColor(0x7d, 0x85, 0x90);

const FONT_BUMP: u32 = 1;
const Y_TICK_LABEL_SIZE: u32 = 12;
const HEADER_SUBTITLE_OFFSET: i32 = 17;
const LEGEND_ROW_H: f64 = 22.0;
const TRANSFER_RATE: f64 = 100_000_000.0;
const TRANSFER_LABEL: &str = "100 MB/s";
const MISA77_LABEL: &str = "C++ misa77 v0.6.0";
const DATASET_SUFFIX: &str = "@1MiB";

const SILESIA: &[&str] = &[
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster",
    "x-ray", "xml",
];
const COMPRESSIBLE: &[&str] = &[
    "dickens", "mozilla", "nci", "ooffice", "osdb", "reymont", "samba", "webster", "xml",
];
const INCOMPRESSIBLE: &[&str] = &["mr", "sao", "x-ray"];
const GROUPS: &[(&str, &[&str])] = &[
    ("Compressible", COMPRESSIBLE),
    ("Incompressible", INCOMPRESSIBLE),
];

const LEVELS: &[Level] = &[
    Level {
        name: "L-1",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level -1",
                decode: "C++ misa77 level -1",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level -1",
                decode: "m77rip (from level -1)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level -1",
                decode: "m77rip paranoid (from level -1)",
            },
        ],
    },
    Level {
        name: "L0",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level 0",
                decode: "C++ misa77 level 0",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level 0",
                decode: "m77rip (from level 0)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level 0",
                decode: "m77rip paranoid (from level 0)",
            },
        ],
    },
    Level {
        name: "L1",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level 1",
                decode: "C++ misa77 level 1",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level 1",
                decode: "m77rip (from level 1)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level 1",
                decode: "m77rip paranoid (from level 1)",
            },
        ],
    },
    Level {
        name: "L2",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level 2",
                decode: "C++ misa77 level 2",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level 2",
                decode: "m77rip (from level 2)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level 2",
                decode: "m77rip paranoid (from level 2)",
            },
        ],
    },
    Level {
        name: "L3",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level 3",
                decode: "C++ misa77 level 3",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level 3",
                decode: "m77rip (from level 3)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level 3",
                decode: "m77rip paranoid (from level 3)",
            },
        ],
    },
    Level {
        name: "L4",
        impls: &[
            ImplPair {
                family: "C++ misa77",
                compress: "C++ misa77 level 4",
                decode: "C++ misa77 level 4",
            },
            ImplPair {
                family: "m77rip",
                compress: "m77rip compress level 4",
                decode: "m77rip (from level 4)",
            },
            ImplPair {
                family: "m77rip paranoid",
                compress: "m77rip paranoid compress level 4",
                decode: "m77rip paranoid (from level 4)",
            },
        ],
    },
];

#[derive(Clone, Copy)]
struct Level {
    name: &'static str,
    impls: &'static [ImplPair],
}

#[derive(Clone, Copy)]
struct ImplPair {
    family: &'static str,
    compress: &'static str,
    decode: &'static str,
}

#[derive(Clone)]
struct CodecStyle {
    key: &'static str,
    label: &'static str,
    color: RGBColor,
    dim: RGBColor,
}

struct Config {
    target: String,
    hw_label: Option<String>,
    styles: Vec<CodecStyle>,
}

#[derive(Deserialize, Clone)]
struct BenchRow {
    codec: String,
    input: String,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse()?;
    let cfg = Config::new();
    let out_dir = args.output_dir.unwrap_or_else(|| {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("doc");
        p.push("charts");
        p.push(&cfg.target);
        p
    });
    std::fs::create_dir_all(&out_dir)?;

    match args.chart {
        ChartKind::All | ChartKind::Summary => draw_summary(&cfg, &out_dir)?,
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum ChartKind {
    All,
    Summary,
}

struct Args {
    chart: ChartKind,
    output_dir: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut chart = ChartKind::All;
        let mut output_dir = None;

        for arg in std::env::args().skip(1) {
            if arg == "-h" || arg == "--help" {
                print_help();
                std::process::exit(0);
            }
            match arg.as_str() {
                "all" => chart = ChartKind::All,
                "summary" => chart = ChartKind::Summary,
                _ => output_dir = Some(PathBuf::from(arg)),
            }
        }

        Ok(Self { chart, output_dir })
    }
}

fn print_help() {
    println!("Usage: m77rip_charts [all|summary] [OUT_DIR]");
}

impl Config {
    fn new() -> Self {
        Self {
            target: std::env::consts::ARCH.into(),
            hw_label: detect_hardware(),
            styles: vec![
                codec("C++ misa77", MISA77_LABEL, 0x60a5fa, 0x4680c4),
                codec("m77rip", "m77rip", 0xf87171, 0xc45050),
                codec("m77rip paranoid", "m77rip paranoid", 0xf472b6, 0xc05a92),
            ],
        }
    }

    fn style(&self, key: &str) -> Option<&CodecStyle> {
        self.styles.iter().find(|s| s.key == key)
    }
}

fn codec(key: &'static str, label: &'static str, color: u32, dim: u32) -> CodecStyle {
    CodecStyle {
        key,
        label,
        color: hex_color(color),
        dim: hex_color(dim),
    }
}

fn hex_color(v: u32) -> RGBColor {
    RGBColor(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

fn detect_hardware() -> Option<String> {
    let hw_conf = read_chart_hw();
    let mut cpu = std::env::var("M77RIP_CPU").ok();
    if cpu.is_none() && cfg!(target_os = "macos") {
        cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    if cpu.is_none() {
        cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| {
                        v.trim()
                            .replace("(R)", "")
                            .replace("(TM)", "")
                            .replace("CPU ", "")
                    })
            });
    }

    let mut extras = Vec::new();
    if std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .is_ok_and(|s| s.trim() == "performance")
    {
        extras.push("performance governor".to_string());
    }
    for (path, off_val) in [
        ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
        ("/sys/devices/system/cpu/cpufreq/boost", "0"),
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            if s.trim() == off_val {
                extras.push("turbo off".to_string());
            }
            break;
        }
    }
    if extras.is_empty()
        && let Ok(hw) = std::env::var("M77RIP_HW_EXTRAS")
    {
        extras.extend(
            hw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    let postfix = std::env::var("M77RIP_HW_POSTFIX")
        .ok()
        .or_else(|| hw_conf.get("postfix").cloned());
    if let Some(postfix) = postfix {
        for value in postfix
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if !extras.iter().any(|existing| existing == &value) {
                extras.push(value);
            }
        }
    }

    let prefix = std::env::var("M77RIP_HW_PREFIX")
        .ok()
        .or_else(|| hw_conf.get("prefix").cloned());
    let cores = std::thread::available_parallelism()
        .ok()
        .map(std::num::NonZero::get);

    if let Some(cpu) = &mut cpu
        && let Some(cores) = cores
    {
        cpu.push_str(&format!(", {cores} cores"));
    }

    let mut parts = Vec::new();
    if let Some(prefix) = prefix.filter(|s| !s.trim().is_empty()) {
        parts.push(prefix);
    }
    match (cpu, extras.is_empty()) {
        (Some(mut cpu), false) => {
            cpu.push_str(", ");
            cpu.push_str(&extras.join(", "));
            parts.push(cpu);
        }
        (Some(cpu), true) => parts.push(cpu),
        (None, false) => parts.push(extras.join(", ")),
        (None, true) => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn read_chart_hw() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for path in [Path::new(".chart_hw"), Path::new("../.chart_hw")] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        break;
    }
    map
}

fn cache_dir(cfg: &Config) -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("m77rip")
        .join(&cfg.target)
}

fn load_cache_dir(path: &Path) -> Vec<BenchRow> {
    let mut rows = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return rows;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect::<Vec<_>>();
    files.sort();

    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Ok(row) = serde_json::from_str::<BenchRow>(line) {
                rows.push(row);
            }
        }
    }
    rows
}

fn draw_summary(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rows = load_cache_dir(&cache_dir(cfg))
        .into_iter()
        .filter(is_dataset_row)
        .collect::<Vec<_>>();
    require_rows(&rows)?;

    let panel_data = LEVELS
        .iter()
        .map(|level| (*level, level_panel_data(&rows, level.impls)))
        .collect::<Vec<_>>();

    let width = 1080;
    let x_left = 70.0;
    let col_gap = 60.0;
    let plot_w = 455.0;
    let left_plot_h = 250.0;
    let right_plot_h = 560.0;
    let y_top = if cfg.hw_label.is_some() { 78.0 } else { 64.0 };
    let left_row_stride = 315.0;
    let right_row_stride = 645.0;
    let left_rows: [f64; 4] = [
        y_top,
        y_top + left_row_stride,
        y_top + 2.0 * left_row_stride,
        y_top + 3.0 * left_row_stride,
    ];
    let right_rows: [f64; 2] = [y_top, y_top + right_row_stride];
    let right_x = x_left + plot_w + col_gap;
    let plot_bottom = (left_rows[3] + left_plot_h).max(right_rows[1] + right_plot_h);
    let height = (plot_bottom + 118.0) as u32;
    let path = out_dir.join("summary.svg");
    let area = root(&path, width, height)?;

    chart_header(
        &area,
        width,
        &format!(
            "12-file Silesia 1MiB slices: misa77 Pipelines @{TRANSFER_LABEL} by Level (lower is better)"
        ),
        cfg.hw_label.as_deref(),
        22,
    )?;

    for (idx, (level, data)) in panel_data.iter().take(4).enumerate() {
        draw_panel(
            &area,
            level.name,
            level.impls,
            data,
            x_left,
            left_rows[idx],
            plot_w,
            left_plot_h,
            4,
            1.15,
        )?;
    }
    for (idx, (level, data)) in panel_data.iter().skip(4).enumerate() {
        draw_panel(
            &area,
            level.name,
            level.impls,
            data,
            right_x,
            right_rows[idx],
            plot_w,
            right_plot_h,
            6,
            1.08,
        )?;
    }

    let leg_y = plot_bottom + 52.0;
    draw_legend(
        &area,
        cfg,
        &["C++ misa77", "m77rip", "m77rip paranoid"],
        width as f64 / 2.0,
        leg_y,
    )?;
    draw_segment_legend(&area, width as f64 / 2.0, leg_y + LEGEND_ROW_H + 8.0)?;

    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn require_rows(rows: &[BenchRow]) -> Result<(), Box<dyn Error>> {
    let mut missing = Vec::new();
    for level in LEVELS {
        for pair in level.impls {
            for input in SILESIA {
                if !rows.iter().any(|r| row_matches(r, input, pair.compress)) {
                    missing.push(format!("{} {input}{DATASET_SUFFIX}", pair.compress));
                }
                if pair.decode != pair.compress
                    && !rows.iter().any(|r| row_matches(r, input, pair.decode))
                {
                    missing.push(format!("{} {input}{DATASET_SUFFIX}", pair.decode));
                }
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let shown = missing.into_iter().take(24).collect::<Vec<_>>().join(", ");
    Err(format!(
        "summary: missing required first-1MiB Silesia cache rows ({shown}). Run default and paranoid `m77rip_bench` with `--features c-reference` first."
    )
    .into())
}

fn is_dataset_row(row: &BenchRow) -> bool {
    row.input
        .strip_suffix(DATASET_SUFFIX)
        .is_some_and(|base| SILESIA.contains(&base))
}

fn row_matches(row: &BenchRow, input: &str, codec: &str) -> bool {
    row.codec == codec && row.input.strip_suffix(DATASET_SUFFIX) == Some(input)
}

fn level_panel_data(
    rows: &[BenchRow],
    impls: &[ImplPair],
) -> BTreeMap<(String, String), (f64, f64, f64)> {
    let mut out = BTreeMap::new();
    for (group, files) in GROUPS {
        for pair in impls {
            if let Some(stack) = aggregate_stack(rows, files, pair.compress, pair.decode) {
                out.insert(((*group).to_string(), pair.family.to_string()), stack);
            }
        }
    }
    out
}

fn aggregate_stack(
    rows: &[BenchRow],
    files: &[&str],
    compress_codec: &str,
    decode_codec: &str,
) -> Option<(f64, f64, f64)> {
    let mut total_input = 0usize;
    let mut total_compressed = 0usize;
    let mut total_compress_ns = 0.0;
    let mut total_decompress_ns = 0.0;

    for input in files {
        let comp = find_result(rows, input, compress_codec)?;
        let decomp = find_result(rows, input, decode_codec)?;
        if comp.compress_ns <= 0.0 || decomp.decompress_ns <= 0.0 {
            return None;
        }
        total_input += comp.input_size;
        total_compressed += comp.compressed_size;
        total_compress_ns += comp.compress_ns;
        total_decompress_ns += decomp.decompress_ns;
    }

    let per_gb = 1e9 / total_input as f64;
    Some((
        total_compress_ns / 1e9 * per_gb,
        (total_compressed as f64 / total_input as f64) * (1e9 / TRANSFER_RATE),
        total_decompress_ns / 1e9 * per_gb,
    ))
}

fn find_result<'a>(rows: &'a [BenchRow], input: &str, codec: &str) -> Option<&'a BenchRow> {
    rows.iter().find(|r| row_matches(r, input, codec))
}

fn draw_panel(
    area: &Area<'_>,
    title: &str,
    impls: &[ImplPair],
    data: &BTreeMap<(String, String), (f64, f64, f64)>,
    x_left: f64,
    y_top: f64,
    plot_w: f64,
    plot_h: f64,
    grid_lines: usize,
    y_padding: f64,
) -> Result<(), Box<dyn Error>> {
    let x_right = x_left + plot_w;
    let y_bot = y_top + plot_h;
    let y_max = data
        .values()
        .map(|(a, b, c)| a + b + c)
        .fold(0.0, f64::max)
        .max(1.0)
        * y_padding;

    text(
        area,
        title,
        px((x_left + x_right) / 2.0),
        px(y_top - 18.0),
        12,
        TEXT,
        HPos::Center,
        true,
    )?;
    draw_y_grid(area, x_left, x_right, y_top, y_bot, y_max, grid_lines)?;
    vtext(
        area,
        "seconds / GB",
        px(x_left - 43.0),
        px((y_top + y_bot) / 2.0),
        10,
        TEXT,
    )?;

    let group_w = plot_w / GROUPS.len() as f64;
    let gap = 5.0;
    let bar_w = (group_w * 0.70 / impls.len() as f64).min(34.0);
    let bars_w = impls.len() as f64 * bar_w + (impls.len() - 1) as f64 * gap;

    for (gi, (group_name, _)) in GROUPS.iter().enumerate() {
        let group_left = x_left + gi as f64 * group_w;
        let bars_left = group_left + (group_w - bars_w) / 2.0;
        for (ci, pair) in impls.iter().enumerate() {
            let Some(stack) = data.get(&((*group_name).to_string(), pair.family.to_string()))
            else {
                continue;
            };
            let Some(style) = family_style(pair.family) else {
                continue;
            };
            draw_stack(
                area,
                bars_left + ci as f64 * (bar_w + gap),
                bar_w,
                y_top,
                y_bot,
                y_max,
                *stack,
                style,
            )?;
        }
        text(
            area,
            *group_name,
            px(group_left + group_w / 2.0),
            px(y_bot + 17.0),
            10,
            TEXT,
            HPos::Center,
            true,
        )?;
    }
    Ok(())
}

fn family_style(family: &str) -> Option<&'static CodecStyle> {
    static CPP: CodecStyle = CodecStyle {
        key: "C++ misa77",
        label: MISA77_LABEL,
        color: RGBColor(0x60, 0xa5, 0xfa),
        dim: RGBColor(0x46, 0x80, 0xc4),
    };
    static M77: CodecStyle = CodecStyle {
        key: "m77rip",
        label: "m77rip",
        color: RGBColor(0xf8, 0x71, 0x71),
        dim: RGBColor(0xc4, 0x50, 0x50),
    };
    static PARANOID: CodecStyle = CodecStyle {
        key: "m77rip paranoid",
        label: "m77rip paranoid",
        color: RGBColor(0xf4, 0x72, 0xb6),
        dim: RGBColor(0xc0, 0x5a, 0x92),
    };
    match family {
        "C++ misa77" => Some(&CPP),
        "m77rip" => Some(&M77),
        "m77rip paranoid" => Some(&PARANOID),
        _ => None,
    }
}

type Area<'a> = DrawingArea<SVGBackend<'a>, Shift>;

fn root(path: &Path, width: u32, height: u32) -> Result<Area<'_>, Box<dyn Error>> {
    let area = SVGBackend::new(path, (width, height)).into_drawing_area();
    area.fill(&BG)?;
    Ok(area)
}

fn finish_svg(path: &Path, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
    let mut svg = std::fs::read_to_string(path)?;
    svg = svg.replacen(
        &format!("<svg width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\""),
        &format!("<svg viewBox=\"0 0 {width} {height}\""),
        1,
    );
    svg = svg.replacen(
        "xmlns=\"http://www.w3.org/2000/svg\"",
        "xmlns=\"http://www.w3.org/2000/svg\" font-family=\"system-ui, -apple-system, sans-serif\"",
        1,
    );
    std::fs::write(path, svg)?;
    Ok(())
}

fn text(
    area: &Area<'_>,
    s: impl Into<String>,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
    hpos: HPos,
    bold: bool,
) -> Result<(), Box<dyn Error>> {
    let mut font = ("sans-serif", size + FONT_BUMP).into_font();
    if bold {
        font = font.style(FontStyle::Bold);
    }
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(hpos, VPos::Center));
    area.draw(&Text::new(s.into(), (x, y), style))?;
    Ok(())
}

fn vtext(
    area: &Area<'_>,
    s: &str,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    let font = ("sans-serif", size + FONT_BUMP)
        .into_font()
        .style(FontStyle::Bold)
        .transform(FontTransform::Rotate270);
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(HPos::Center, VPos::Center));
    area.draw(&Text::new(s.to_string(), (x, y), style))?;
    Ok(())
}

fn rect(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    area.draw(&Rectangle::new(
        [(px(x1), px(y1)), (px(x2), px(y2))],
        ShapeStyle::from(&color).filled(),
    ))?;
    Ok(())
}

fn line(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
    width: u32,
) -> Result<(), Box<dyn Error>> {
    area.draw(&PathElement::new(
        vec![(px(x1), px(y1)), (px(x2), px(y2))],
        color.stroke_width(width),
    ))?;
    Ok(())
}

fn draw_stack(
    area: &Area<'_>,
    x: f64,
    width: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    parts: (f64, f64, f64),
    style: &CodecStyle,
) -> Result<(), Box<dyn Error>> {
    let (comp, transfer, decomp) = parts;
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    rect(area, x, map_y(comp), x + width, p_bot, style.color)?;
    rect(
        area,
        x,
        map_y(comp + transfer),
        x + width,
        map_y(comp),
        style.dim,
    )?;
    rect(
        area,
        x,
        map_y(comp + transfer + decomp),
        x + width,
        map_y(comp + transfer),
        style.color,
    )?;
    Ok(())
}

fn draw_y_grid(
    area: &Area<'_>,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    target_lines: usize,
) -> Result<(), Box<dyn Error>> {
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    let step = nice_step(y_max, target_lines);
    let mut v = step;
    while v <= y_max {
        let yy = map_y(v);
        line(area, x_left, yy, x_right, yy, GRID, 1)?;
        text(
            area,
            format!("{v:.0}"),
            px(x_left - 7.0),
            px(yy),
            Y_TICK_LABEL_SIZE,
            MUTED,
            HPos::Right,
            false,
        )?;
        v += step;
    }
    line(area, x_left, p_bot, x_right, p_bot, AXIS, 2)?;
    Ok(())
}

fn chart_header(
    area: &Area<'_>,
    width: u32,
    title: &str,
    hw: Option<&str>,
    y: i32,
) -> Result<(), Box<dyn Error>> {
    let mid = (width / 2) as i32;
    text(area, title, mid, y, 14, TEXT, HPos::Center, true)?;
    if let Some(hw) = hw {
        text(
            area,
            hw,
            mid,
            y + HEADER_SUBTITLE_OFFSET,
            10,
            MUTED,
            HPos::Center,
            false,
        )?;
    }
    Ok(())
}

fn draw_legend(
    area: &Area<'_>,
    cfg: &Config,
    items: &[&str],
    mid_x: f64,
    y: f64,
) -> Result<(), Box<dyn Error>> {
    let widths = items
        .iter()
        .filter_map(|key| cfg.style(key))
        .map(|style| style.label.chars().count() as f64 * 7.0 + 34.0)
        .collect::<Vec<_>>();
    let total = widths.iter().sum::<f64>() + (items.len() - 1) as f64 * 28.0;
    let mut x = mid_x - total / 2.0;
    for (key, width) in items.iter().zip(widths) {
        let Some(style) = cfg.style(key) else {
            continue;
        };
        rect(area, x, y - 6.0, x + 12.0, y + 6.0, style.color)?;
        text(
            area,
            style.label,
            px(x + 18.0),
            px(y),
            10,
            TEXT,
            HPos::Left,
            false,
        )?;
        x += width + 28.0;
    }
    Ok(())
}

fn draw_segment_legend(area: &Area<'_>, mid_x: f64, y: f64) -> Result<(), Box<dyn Error>> {
    text(
        area,
        "bright = encode + decode",
        px(mid_x - 205.0),
        px(y),
        9,
        TEXT,
        HPos::Left,
        false,
    )?;
    text(
        area,
        format!("dim = transfer @{TRANSFER_LABEL}"),
        px(mid_x + 25.0),
        px(y),
        9,
        MUTED,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn px(v: f64) -> i32 {
    v.round() as i32
}

fn nice_step(max_val: f64, target_lines: usize) -> f64 {
    if max_val <= 0.0 {
        return 1.0;
    }
    let raw = max_val / target_lines as f64;
    let mag = 10.0_f64.powf(raw.max(1e-9).log10().floor());
    for s in [1.0, 2.0, 5.0, 10.0] {
        let step = s * mag;
        if max_val / step <= target_lines as f64 + 1.0 {
            return step;
        }
    }
    mag * 10.0
}
