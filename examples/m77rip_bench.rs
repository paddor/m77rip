#![deny(unsafe_op_in_unsafe_fn)]

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
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut ts) };
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
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
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
                let size = std::fs::metadata(path).map_or(0, |m| m.len());
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

/// A benchmarked codec and level. Its name keys the result cache.
#[derive(Clone, Copy)]
enum Codec {
    Cpp(i8),
    CppSafe,
    Compress(i8),
    Decode(i8),
}

impl Codec {
    fn all() -> Vec<Self> {
        let mut all: Vec<Self> = LEVELS.iter().map(|&l| Self::Cpp(l)).collect();
        all.push(Self::CppSafe);
        all.extend(LEVELS.iter().map(|&l| Self::Compress(l)));
        all.extend(LEVELS.iter().map(|&l| Self::Decode(l)));
        all
    }

    fn name(self) -> String {
        match self {
            Self::Cpp(level) => format!("C++ misa77 level {level}"),
            Self::CppSafe => "C++ misa77 safe level 0".to_string(),
            Self::Compress(level) => format!("m77rip compress level {level}"),
            Self::Decode(level) => format!("m77rip (from level {level})"),
        }
    }

    fn run(self, data: &[u8], name: &str, target_ns: u64) -> BenchResult {
        match self {
            Self::Cpp(level) => bench_c_misa77(data, name, target_ns, level),
            Self::CppSafe => bench_c_misa77_safe(data, name, target_ns, 0),
            Self::Compress(level) => bench_m77rip_compress(data, name, target_ns, level),
            Self::Decode(level) => bench_m77rip(data, name, target_ns, level),
        }
    }
}

/// Command-line options.
struct Args {
    only: Vec<String>,
    files: Vec<String>,
    max_bytes: Option<usize>,
}

impl Args {
    /// Parse the command line. Returns `None` after printing help.
    fn parse() -> Option<Self> {
        let mut args = Self {
            only: Vec::new(),
            files: Vec::new(),
            max_bytes: Some(DEFAULT_MAX_BYTES),
        };
        let mut words = std::env::args().skip(1);
        while let Some(arg) = words.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    println!(
                        "usage: m77rip_bench [--impl TEXT] [--files a,b] [--max-bytes N|1MiB|1M]"
                    );
                    println!("default: --max-bytes 1MiB");
                    return None;
                }
                "--impl" => args.only.extend(words.next()),
                "--files" => {
                    let files = words.next().unwrap_or_default();
                    args.files.extend(files.split(',').map(str::to_string));
                }
                "--max-bytes" => {
                    let value = words.next().unwrap_or_default();
                    let bytes = parse_size_arg(&value)
                        .unwrap_or_else(|| panic!("invalid --max-bytes value: {value}"));
                    args.max_bytes = Some(bytes);
                }
                _ => {}
            }
        }
        Some(args)
    }

    /// Corpus files to run, with their result names.
    fn inputs(&self) -> Vec<(&'static str, String)> {
        ALL_FILES
            .iter()
            .filter_map(|path| {
                let base = path.rsplit('/').next().unwrap();
                if !self.files.is_empty() && !self.files.iter().any(|f| f == base) {
                    return None;
                }
                let name = match self.max_bytes {
                    Some(max_bytes) => format!("{base}@{}", size_label(max_bytes)),
                    None => base.to_string(),
                };
                Some((*path, name))
            })
            .collect()
    }

    fn should_run(&self, codec: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|o| codec.contains(o.as_str()))
    }
}

fn main() {
    ensure_corpus();
    let Some(args) = Args::parse() else {
        return;
    };
    let codecs = Codec::all();
    let names: Vec<String> = codecs.iter().map(|c| c.name()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let cached = load_cache(&names);
    let mut results = Vec::new();
    for (path, name) in args.inputs() {
        let Ok(mut data) = std::fs::read(path) else {
            eprintln!("skipping {path}: not found");
            continue;
        };
        if let Some(max_bytes) = args.max_bytes {
            data.truncate(max_bytes.min(data.len()));
        }
        for &codec in &codecs {
            let codec_name = codec.name();
            if args.should_run(&codec_name) {
                eprintln!("  {codec_name} x {name}: benchmarking...");
                results.push(codec.run(&data, &name, 20_000_000));
            } else if let Some(c) = cached
                .iter()
                .find(|c| c.codec == codec_name && c.input_name == name)
            {
                eprintln!("  {codec_name} x {name}: cached");
                results.push(c.clone());
            } else {
                eprintln!("  {codec_name} x {name}: skipped");
            }
        }
    }
    save_cache(&results, &names);
    print_table("Decompression", &args, &results, Codec::Decode, |r| {
        r.decompress_ns
    });
    print_table("Compression", &args, &results, Codec::Compress, |r| {
        r.compress_ns
    });
}

/// Print C++ against m77rip speeds per input and level. `ns` picks the
/// timing; zero means not measured.
fn print_table(
    title: &str,
    args: &Args,
    results: &[BenchResult],
    rust: fn(i8) -> Codec,
    ns: fn(&BenchResult) -> f64,
) {
    println!();
    println!("=== {title} ===");
    println!(
        "{:<12} {:>5} {:>12} {:>12} {:>8}",
        "input", "level", "C++", "m77rip", "ratio"
    );
    println!("{}", "-".repeat(55));
    let mbps = |r: Option<&BenchResult>| match r {
        Some(r) if ns(r) > 0.0 => format!("{:.0} MB/s", r.input_size as f64 / ns(r) * 1000.0),
        _ => "-".to_string(),
    };
    for (_, name) in args.inputs() {
        let find = |codec: Codec| {
            let codec = codec.name();
            results
                .iter()
                .find(|r| r.codec == codec && r.input_name == name)
        };
        for &level in LEVELS {
            let (cpp, rust) = (find(Codec::Cpp(level)), find(rust(level)));
            let ratio = match (cpp, rust) {
                (Some(c), Some(r)) if ns(c) > 0.0 && ns(r) > 0.0 => {
                    format!("{:.2}x", ns(c) / ns(r))
                }
                _ => "-".to_string(),
            };
            println!(
                "{name:<12} {level:>5} {:>12} {:>12} {ratio:>8}",
                mbps(cpp),
                mbps(rust)
            );
        }
    }
}
