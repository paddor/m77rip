#![deny(unsafe_op_in_unsafe_fn)]

extern crate libc;

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

// SAFETY: These declarations match the C wrapper ABI. Calls validate pointer
// lifetimes and capacities at each call site below.
unsafe extern "C" {
    fn misa77_compress_bound(src_size: u64, level: i8) -> u64;
    fn misa77_compress(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64, level: i8)
    -> u64;
    fn misa77_decompress(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64) -> u64;
    fn misa77_decompress_safe(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64) -> u64;
}

fn cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid writable pointer to initialized storage.
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[derive(Clone)]
struct BenchResult {
    codec: String,
    input_name: String,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

impl BenchResult {
    fn to_json(&self) -> String {
        format!(
            r#"{{"codec": "{}", "input": "{}", "input_size": {}, "compressed_size": {}, "compress_ns": {:.1}, "decompress_ns": {:.1}}}"#,
            self.codec,
            self.input_name,
            self.input_size,
            self.compressed_size,
            self.compress_ns,
            self.decompress_ns
        )
    }

    fn from_json(line: &str) -> Option<Self> {
        let line = line.trim().trim_matches(',');
        if line == "[" || line == "]" || line.is_empty() {
            return None;
        }
        let get = |key: &str| -> Option<String> {
            let prefix = format!("\"{key}\": ");
            let start = line.find(&prefix)? + prefix.len();
            let rest = &line[start..];
            if let Some(stripped) = rest.strip_prefix('"') {
                let end = stripped.find('"')?;
                Some(stripped[..end].to_string())
            } else {
                let end = rest.find([',', '}']).unwrap_or(rest.len());
                Some(rest[..end].to_string())
            }
        };
        Some(BenchResult {
            codec: get("codec")?,
            input_name: get("input")?,
            input_size: get("input_size")?.parse().ok()?,
            compressed_size: get("compressed_size")?.parse().ok()?,
            compress_ns: get("compress_ns")?.parse().ok()?,
            decompress_ns: get("decompress_ns")?.parse().ok()?,
        })
    }
}

fn bench_loop<F: FnMut()>(warmup: usize, target_ns: u64, rounds: usize, mut f: F) -> f64 {
    for _ in 0..warmup {
        f();
    }
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let mut iters = 0u64;
        let start = cpu_nanos();
        loop {
            std::hint::black_box(&mut f)();
            iters += 1;
            if cpu_nanos() - start >= target_ns {
                break;
            }
        }
        let elapsed = cpu_nanos() - start;
        let ns_per_op = elapsed as f64 / iters as f64;
        if ns_per_op < best {
            best = ns_per_op;
        }
    }
    best
}

fn c_misa77_compress(data: &[u8], level: i8) -> Vec<u8> {
    // SAFETY: C function has no pointer arguments and accepts any u64 size.
    let bound = unsafe { misa77_compress_bound(data.len() as u64, level) } as usize;
    let mut out = vec![0u8; bound];
    // SAFETY: Pointers come from live borrowed slices. `out` has the bound
    // capacity passed to the C compressor.
    let written = unsafe {
        misa77_compress(
            data.as_ptr(),
            data.len() as u64,
            out.as_mut_ptr(),
            out.len() as u64,
            level,
        )
    } as usize;
    assert!(written > 0, "C++ misa77 compress failed");
    out.truncate(written);
    out
}

#[allow(dead_code)]
fn c_misa77_decompress(compressed: &[u8], original_size: usize) -> Vec<u8> {
    let mut out = vec![0u8; original_size];
    // SAFETY: Pointers come from live benchmark buffers with matching lengths
    // and capacities.
    let written = unsafe {
        misa77_decompress(
            compressed.as_ptr(),
            compressed.len() as u64,
            out.as_mut_ptr(),
            out.len() as u64,
        )
    } as usize;
    assert_eq!(
        written, original_size,
        "C++ misa77 decompress size mismatch"
    );
    out
}

fn bench_c_misa77(data: &[u8], name: &str, target_ns: u64, level: i8) -> BenchResult {
    let compressed = c_misa77_compress(data, level);
    let mut decomp_buf = vec![0u8; data.len()];

    let compress_ns = {
        // SAFETY: C function has no pointer arguments and accepts any u64 size.
        let bound = unsafe { misa77_compress_bound(data.len() as u64, level) } as usize;
        let mut comp_buf = vec![0u8; bound];
        // SAFETY: Pointers reference live benchmark buffers with matching
        // lengths and capacities.
        bench_loop(3, target_ns, 10, || unsafe {
            misa77_compress(
                data.as_ptr(),
                data.len() as u64,
                comp_buf.as_mut_ptr(),
                comp_buf.len() as u64,
                level,
            );
        })
    };

    // SAFETY: Pointers reference live benchmark buffers with matching lengths
    // and capacities.
    let decompress_ns = bench_loop(3, target_ns, 10, || unsafe {
        misa77_decompress(
            compressed.as_ptr(),
            compressed.len() as u64,
            decomp_buf.as_mut_ptr(),
            decomp_buf.len() as u64,
        );
    });

    BenchResult {
        codec: format!("C++ misa77 level {level}"),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn m77rip_level_label(level: i8) -> String {
    match level {
        -1..=4 => format!("level {level}"),
        _ => unreachable!(),
    }
}

fn bench_m77rip_compress(data: &[u8], name: &str, target_ns: u64, level: i8) -> BenchResult {
    let compressed = m77rip::compress_level(data, level).unwrap();
    let label = m77rip_level_label(level);

    // Verify roundtrip
    let decompressed = m77rip::decompress(&compressed, data.len()).unwrap();
    assert_eq!(
        &decompressed, data,
        "m77rip compress {label} roundtrip mismatch on {name}"
    );

    let mut comp_buf = vec![0u8; m77rip::compress_bound_level(data.len(), level).unwrap()];
    let compress_ns = bench_loop(3, target_ns, 10, || {
        let _ = m77rip::compress_into_level(
            std::hint::black_box(data),
            std::hint::black_box(&mut comp_buf),
            level,
        );
    });

    BenchResult {
        codec: format!("m77rip compress {label}"),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns: 0.0,
    }
}

fn bench_m77rip(data: &[u8], name: &str, target_ns: u64, level: i8) -> BenchResult {
    let compressed = m77rip::compress_level(data, level).unwrap();
    let label = m77rip_level_label(level);
    let original_size = data.len();
    let mut decomp_buf = vec![0u8; original_size];

    // Verify our decoder produces correct output
    let result = m77rip::decompress(&compressed, original_size).unwrap();
    assert_eq!(&result, data, "m77rip decompress mismatch on {name}");

    let decompress_ns = bench_loop(3, target_ns, 10, || {
        let _ = m77rip::decompress_into(
            std::hint::black_box(&compressed),
            std::hint::black_box(&mut decomp_buf),
        );
    });

    BenchResult {
        codec: format!("m77rip (from {label})"),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns: 0.0,
        decompress_ns,
    }
}

fn bench_c_misa77_safe(data: &[u8], name: &str, target_ns: u64, level: i8) -> BenchResult {
    let compressed = c_misa77_compress(data, level);
    let mut decomp_buf = vec![0u8; data.len()];

    // SAFETY: Pointers reference live benchmark buffers with matching lengths
    // and capacities.
    let decompress_ns = bench_loop(3, target_ns, 10, || unsafe {
        misa77_decompress_safe(
            compressed.as_ptr(),
            compressed.len() as u64,
            decomp_buf.as_mut_ptr(),
            decomp_buf.len() as u64,
        );
    });

    BenchResult {
        codec: format!("C++ misa77 safe level {level}"),
        input_name: name.to_string(),
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns: 0.0,
        decompress_ns,
    }
}

fn arch() -> &'static str {
    std::env::consts::ARCH
}

fn cache_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("m77rip")
        .join(arch());
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn codec_cache_path(codec: &str) -> PathBuf {
    let filename = codec.replace(' ', "_").replace(['(', ')'], "");
    cache_dir().join(format!("{filename}.jsonl"))
}

fn load_cache(codecs: &[&str]) -> Vec<BenchResult> {
    let mut results = Vec::new();
    for codec in codecs {
        let path = codec_cache_path(codec);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        results.extend(content.lines().filter_map(BenchResult::from_json));
    }
    results
}

fn save_cache(results: &[BenchResult], codecs: &[&str]) {
    for codec in codecs {
        let new_entries: Vec<_> = results.iter().filter(|r| r.codec == *codec).collect();
        if new_entries.is_empty() {
            continue;
        }
        let path = codec_cache_path(codec);
        let mut entries = match std::fs::read_to_string(&path) {
            Ok(content) => content
                .lines()
                .filter_map(BenchResult::from_json)
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        let replaced_inputs = new_entries
            .iter()
            .map(|r| r.input_name.as_str())
            .collect::<HashSet<_>>();
        entries.retain(|r| !replaced_inputs.contains(r.input_name.as_str()));
        entries.extend(new_entries.into_iter().cloned());
        let mut f = std::fs::File::create(&path).unwrap();
        for r in &entries {
            writeln!(f, "{}", r.to_json()).unwrap();
        }
        eprintln!("cached {} results to {}", entries.len(), path.display());
    }
}

const SILESIA_DOWNLOADS: &[(&str, &str)] = &[
    (
        "corpus/silesia/dickens",
        "https://sun.aei.polsl.pl/~sdeor/corpus/dickens.bz2",
    ),
    (
        "corpus/silesia/mozilla",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mozilla.bz2",
    ),
    (
        "corpus/silesia/mr",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mr.bz2",
    ),
    (
        "corpus/silesia/nci",
        "https://sun.aei.polsl.pl/~sdeor/corpus/nci.bz2",
    ),
    (
        "corpus/silesia/ooffice",
        "https://sun.aei.polsl.pl/~sdeor/corpus/ooffice.bz2",
    ),
    (
        "corpus/silesia/osdb",
        "https://sun.aei.polsl.pl/~sdeor/corpus/osdb.bz2",
    ),
    (
        "corpus/silesia/reymont",
        "https://sun.aei.polsl.pl/~sdeor/corpus/reymont.bz2",
    ),
    (
        "corpus/silesia/samba",
        "https://sun.aei.polsl.pl/~sdeor/corpus/samba.bz2",
    ),
    (
        "corpus/silesia/sao",
        "https://sun.aei.polsl.pl/~sdeor/corpus/sao.bz2",
    ),
    (
        "corpus/silesia/webster",
        "https://sun.aei.polsl.pl/~sdeor/corpus/webster.bz2",
    ),
    (
        "corpus/silesia/x-ray",
        "https://sun.aei.polsl.pl/~sdeor/corpus/x-ray.bz2",
    ),
    (
        "corpus/silesia/xml",
        "https://sun.aei.polsl.pl/~sdeor/corpus/xml.bz2",
    ),
];

fn ensure_corpus() {
    for &(path, url) in SILESIA_DOWNLOADS {
        if std::fs::metadata(path).is_ok() {
            continue;
        }
        eprintln!("downloading {url} ...");
        let dir = PathBuf::from(path).parent().unwrap().to_owned();
        std::fs::create_dir_all(&dir).ok();
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("curl -fSL '{url}' | bzip2 -d > '{path}'"))
            .status();
        match status {
            Ok(s) if s.success() => {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  saved {path} ({size} bytes)");
            }
            _ => {
                eprintln!("  failed to download {path}, skipping");
                std::fs::remove_file(path).ok();
            }
        }
    }
}

fn parse_size_arg(value: &str) -> Option<usize> {
    let (number, multiplier) = match value
        .strip_suffix("KiB")
        .or_else(|| value.strip_suffix("kib"))
    {
        Some(number) => (number, 1024usize),
        None => match value
            .strip_suffix("MiB")
            .or_else(|| value.strip_suffix("mib"))
        {
            Some(number) => (number, 1024usize * 1024),
            None => match value
                .strip_suffix("GiB")
                .or_else(|| value.strip_suffix("gib"))
            {
                Some(number) => (number, 1024usize * 1024 * 1024),
                None => match value.strip_suffix('K').or_else(|| value.strip_suffix('k')) {
                    Some(number) => (number, 1000usize),
                    None => match value.strip_suffix('M').or_else(|| value.strip_suffix('m')) {
                        Some(number) => (number, 1000usize * 1000),
                        None => match value.strip_suffix('G').or_else(|| value.strip_suffix('g')) {
                            Some(number) => (number, 1000usize * 1000 * 1000),
                            None => (value, 1usize),
                        },
                    },
                },
            },
        },
    };
    number.parse::<usize>().ok()?.checked_mul(multiplier)
}

fn size_label(bytes: usize) -> String {
    if bytes.is_multiple_of(1024 * 1024) {
        format!("{}MiB", bytes / 1024 / 1024)
    } else if bytes.is_multiple_of(1024) {
        format!("{}KiB", bytes / 1024)
    } else {
        bytes.to_string()
    }
}

const ALL_FILES: &[&str] = &[
    "corpus/silesia/dickens",
    "corpus/silesia/mozilla",
    "corpus/silesia/mr",
    "corpus/silesia/nci",
    "corpus/silesia/ooffice",
    "corpus/silesia/osdb",
    "corpus/silesia/reymont",
    "corpus/silesia/samba",
    "corpus/silesia/sao",
    "corpus/silesia/webster",
    "corpus/silesia/x-ray",
    "corpus/silesia/xml",
];

const DEFAULT_MAX_BYTES: usize = 1024 * 1024;
const LEVELS: &[i8] = &[-1, 0, 1, 2, 3, 4];

const CPP_MISA77_M1: &str = "C++ misa77 level -1";
const CPP_MISA77_0: &str = "C++ misa77 level 0";
const CPP_MISA77_1: &str = "C++ misa77 level 1";
const CPP_MISA77_2: &str = "C++ misa77 level 2";
const CPP_MISA77_3: &str = "C++ misa77 level 3";
const CPP_MISA77_4: &str = "C++ misa77 level 4";
const CPP_MISA77_SAFE_0: &str = "C++ misa77 safe level 0";

#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_M1: &str = "m77rip compress level -1";
#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_0: &str = "m77rip compress level 0";
#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_1: &str = "m77rip compress level 1";
#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_2: &str = "m77rip compress level 2";
#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_3: &str = "m77rip compress level 3";
#[cfg(not(feature = "paranoid"))]
const M77RIP_COMPRESS_4: &str = "m77rip compress level 4";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_M1: &str = "m77rip (from level -1)";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_0: &str = "m77rip (from level 0)";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_1: &str = "m77rip (from level 1)";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_2: &str = "m77rip (from level 2)";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_3: &str = "m77rip (from level 3)";
#[cfg(not(feature = "paranoid"))]
const M77RIP_DECODE_4: &str = "m77rip (from level 4)";

#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_M1: &str = "m77rip paranoid compress level -1";
#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_0: &str = "m77rip paranoid compress level 0";
#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_1: &str = "m77rip paranoid compress level 1";
#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_2: &str = "m77rip paranoid compress level 2";
#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_3: &str = "m77rip paranoid compress level 3";
#[cfg(feature = "paranoid")]
const M77RIP_COMPRESS_4: &str = "m77rip paranoid compress level 4";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_M1: &str = "m77rip paranoid (from level -1)";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_0: &str = "m77rip paranoid (from level 0)";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_1: &str = "m77rip paranoid (from level 1)";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_2: &str = "m77rip paranoid (from level 2)";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_3: &str = "m77rip paranoid (from level 3)";
#[cfg(feature = "paranoid")]
const M77RIP_DECODE_4: &str = "m77rip paranoid (from level 4)";

const CODECS: &[&str] = &[
    CPP_MISA77_M1,
    CPP_MISA77_0,
    CPP_MISA77_1,
    CPP_MISA77_2,
    CPP_MISA77_3,
    CPP_MISA77_4,
    CPP_MISA77_SAFE_0,
    M77RIP_COMPRESS_M1,
    M77RIP_COMPRESS_0,
    M77RIP_COMPRESS_1,
    M77RIP_COMPRESS_2,
    M77RIP_COMPRESS_3,
    M77RIP_COMPRESS_4,
    M77RIP_DECODE_M1,
    M77RIP_DECODE_0,
    M77RIP_DECODE_1,
    M77RIP_DECODE_2,
    M77RIP_DECODE_3,
    M77RIP_DECODE_4,
];

fn cpp_codec(level: i8) -> &'static str {
    match level {
        -1 => CPP_MISA77_M1,
        0 => CPP_MISA77_0,
        1 => CPP_MISA77_1,
        2 => CPP_MISA77_2,
        3 => CPP_MISA77_3,
        4 => CPP_MISA77_4,
        _ => unreachable!(),
    }
}

fn m77rip_compress_codec(level: i8) -> &'static str {
    match level {
        -1 => M77RIP_COMPRESS_M1,
        0 => M77RIP_COMPRESS_0,
        1 => M77RIP_COMPRESS_1,
        2 => M77RIP_COMPRESS_2,
        3 => M77RIP_COMPRESS_3,
        4 => M77RIP_COMPRESS_4,
        _ => unreachable!(),
    }
}

fn m77rip_decode_codec(level: i8) -> &'static str {
    match level {
        -1 => M77RIP_DECODE_M1,
        0 => M77RIP_DECODE_0,
        1 => M77RIP_DECODE_1,
        2 => M77RIP_DECODE_2,
        3 => M77RIP_DECODE_3,
        4 => M77RIP_DECODE_4,
        _ => unreachable!(),
    }
}

fn main() {
    ensure_corpus();

    let args: Vec<String> = std::env::args().collect();
    let mut only: Vec<String> = Vec::new();
    let mut file_filter: Vec<String> = Vec::new();
    let mut max_bytes: Option<usize> = Some(DEFAULT_MAX_BYTES);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!("usage: m77rip_bench [--impl TEXT] [--files a,b] [--max-bytes N|1MiB|1M]");
                println!("default: --max-bytes 1MiB");
                return;
            }
            "--impl" => {
                i += 1;
                if i < args.len() {
                    only.push(args[i].clone());
                }
            }
            "--files" => {
                i += 1;
                if i < args.len() {
                    file_filter.extend(args[i].split(',').map(|s| s.to_string()));
                }
            }
            "--max-bytes" => {
                i += 1;
                if i < args.len() {
                    max_bytes = Some(
                        parse_size_arg(&args[i])
                            .unwrap_or_else(|| panic!("invalid --max-bytes value: {}", args[i])),
                    );
                }
            }
            _ => {}
        }
        i += 1;
    }

    let target_ns = 20_000_000u64;
    let cached = load_cache(CODECS);
    let mut results: Vec<BenchResult> = Vec::new();

    for path in ALL_FILES {
        let base_name = path.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == base_name) {
            continue;
        }

        let mut data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("skipping {path}: not found");
                continue;
            }
        };
        if let Some(max_bytes) = max_bytes {
            data.truncate(max_bytes.min(data.len()));
        }
        let name = match max_bytes {
            Some(max_bytes) => format!("{base_name}@{}", size_label(max_bytes)),
            None => base_name.to_string(),
        };

        for &codec in CODECS {
            let should_run = only.is_empty() || only.iter().any(|o| codec.contains(o.as_str()));

            if !should_run
                && let Some(c) = cached
                    .iter()
                    .find(|c| c.codec == codec && c.input_name == name)
            {
                eprintln!("  {codec} x {name}: cached");
                results.push(c.clone());
                continue;
            }
            if !should_run {
                eprintln!("  {codec} x {name}: skipped");
                continue;
            }

            eprintln!("  {codec} x {name}: benchmarking...");
            let r = match codec {
                CPP_MISA77_M1 => bench_c_misa77(&data, &name, target_ns, -1),
                CPP_MISA77_0 => bench_c_misa77(&data, &name, target_ns, 0),
                CPP_MISA77_1 => bench_c_misa77(&data, &name, target_ns, 1),
                CPP_MISA77_2 => bench_c_misa77(&data, &name, target_ns, 2),
                CPP_MISA77_3 => bench_c_misa77(&data, &name, target_ns, 3),
                CPP_MISA77_4 => bench_c_misa77(&data, &name, target_ns, 4),
                CPP_MISA77_SAFE_0 => bench_c_misa77_safe(&data, &name, target_ns, 0),
                M77RIP_COMPRESS_M1 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, -1);
                    r.codec = M77RIP_COMPRESS_M1.to_string();
                    r
                }
                M77RIP_COMPRESS_0 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, 0);
                    r.codec = M77RIP_COMPRESS_0.to_string();
                    r
                }
                M77RIP_COMPRESS_1 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, 1);
                    r.codec = M77RIP_COMPRESS_1.to_string();
                    r
                }
                M77RIP_COMPRESS_2 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, 2);
                    r.codec = M77RIP_COMPRESS_2.to_string();
                    r
                }
                M77RIP_COMPRESS_3 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, 3);
                    r.codec = M77RIP_COMPRESS_3.to_string();
                    r
                }
                M77RIP_COMPRESS_4 => {
                    let mut r = bench_m77rip_compress(&data, &name, target_ns, 4);
                    r.codec = M77RIP_COMPRESS_4.to_string();
                    r
                }
                M77RIP_DECODE_M1 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, -1);
                    r.codec = M77RIP_DECODE_M1.to_string();
                    r
                }
                M77RIP_DECODE_0 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, 0);
                    r.codec = M77RIP_DECODE_0.to_string();
                    r
                }
                M77RIP_DECODE_1 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, 1);
                    r.codec = M77RIP_DECODE_1.to_string();
                    r
                }
                M77RIP_DECODE_2 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, 2);
                    r.codec = M77RIP_DECODE_2.to_string();
                    r
                }
                M77RIP_DECODE_3 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, 3);
                    r.codec = M77RIP_DECODE_3.to_string();
                    r
                }
                M77RIP_DECODE_4 => {
                    let mut r = bench_m77rip(&data, &name, target_ns, 4);
                    r.codec = M77RIP_DECODE_4.to_string();
                    r
                }
                _ => unreachable!(),
            };
            results.push(r);
        }
    }

    save_cache(&results, CODECS);

    println!();
    println!("=== Decompression ===");
    println!(
        "{:<12} {:>5} {:>12} {:>12} {:>8}",
        "input", "level", "C++", "m77rip", "ratio"
    );
    println!("{}", "-".repeat(55));

    let fmt_mbps_decomp = |r: Option<&BenchResult>| -> String {
        match r {
            Some(r) if r.decompress_ns > 0.0 => {
                let mbps = r.input_size as f64 / r.decompress_ns * 1000.0;
                format!("{mbps:.0} MB/s")
            }
            _ => "-".to_string(),
        }
    };

    let fmt_mbps_comp = |r: Option<&BenchResult>| -> String {
        match r {
            Some(r) if r.compress_ns > 0.0 => {
                let mbps = r.input_size as f64 / r.compress_ns * 1000.0;
                format!("{mbps:.0} MB/s")
            }
            _ => "-".to_string(),
        }
    };

    let fmt_ratio_decomp = |cpp: Option<&BenchResult>, rust: Option<&BenchResult>| -> String {
        match (cpp, rust) {
            (Some(c), Some(r)) if c.decompress_ns > 0.0 && r.decompress_ns > 0.0 => {
                let ratio = c.decompress_ns / r.decompress_ns;
                format!("{ratio:.2}x")
            }
            _ => "-".to_string(),
        }
    };

    let fmt_ratio_comp = |cpp: Option<&BenchResult>, rust: Option<&BenchResult>| -> String {
        match (cpp, rust) {
            (Some(c), Some(r)) if c.compress_ns > 0.0 && r.compress_ns > 0.0 => {
                let ratio = c.compress_ns / r.compress_ns;
                format!("{ratio:.2}x")
            }
            _ => "-".to_string(),
        }
    };

    for path in ALL_FILES {
        let base_name = path.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == base_name) {
            continue;
        }
        let name = match max_bytes {
            Some(max_bytes) => format!("{base_name}@{}", size_label(max_bytes)),
            None => base_name.to_string(),
        };

        let find = |codec: &str| -> Option<&BenchResult> {
            results
                .iter()
                .find(|r| r.codec == codec && r.input_name == name)
        };

        for &level in LEVELS {
            let cpp = cpp_codec(level);
            let rust = m77rip_decode_codec(level);
            println!(
                "{:<12} {:>5} {:>12} {:>12} {:>8}",
                name,
                level,
                fmt_mbps_decomp(find(cpp)),
                fmt_mbps_decomp(find(rust)),
                fmt_ratio_decomp(find(cpp), find(rust)),
            );
        }
    }

    println!();
    println!("=== Compression ===");
    println!(
        "{:<12} {:>5} {:>12} {:>12} {:>8}",
        "input", "level", "C++", "m77rip", "ratio"
    );
    println!("{}", "-".repeat(55));

    for path in ALL_FILES {
        let base_name = path.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == base_name) {
            continue;
        }
        let name = match max_bytes {
            Some(max_bytes) => format!("{base_name}@{}", size_label(max_bytes)),
            None => base_name.to_string(),
        };

        let find = |codec: &str| -> Option<&BenchResult> {
            results
                .iter()
                .find(|r| r.codec == codec && r.input_name == name)
        };

        for &level in LEVELS {
            let cpp = cpp_codec(level);
            let rust = m77rip_compress_codec(level);
            println!(
                "{:<12} {:>5} {:>12} {:>12} {:>8}",
                name,
                level,
                fmt_mbps_comp(find(cpp)),
                fmt_mbps_comp(find(rust)),
                fmt_ratio_comp(find(cpp), find(rust)),
            );
        }
    }
}
