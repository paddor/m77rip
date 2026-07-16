#![deny(unsafe_op_in_unsafe_fn)]

extern crate libc;

use std::path::PathBuf;
use std::process::Command;

// SAFETY: These declarations match the C wrapper ABI. Calls validate pointer
// lifetimes and capacities at each call site below.
unsafe extern "C" {
    fn misa77_compress(src: *const u8, src_size: u64, dst: *mut u8, dst_cap: u64, level: u8)
    -> u64;
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

fn c_compress_into(data: &[u8], dst: &mut [u8], level: u8) -> usize {
    // SAFETY: Pointers come from live borrowed slices. `dst` has the exact
    // capacity passed to the C function.
    (unsafe {
        misa77_compress(
            data.as_ptr(),
            data.len() as u64,
            dst.as_mut_ptr(),
            dst.len() as u64,
            level,
        )
    }) as usize
}

const SILESIA_DOWNLOADS: &[(&str, &str)] = &[
    (
        "corpus/silesia/dickens",
        "https://sun.aei.polsl.pl/~sdeor/corpus/dickens.bz2",
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
        let dir = PathBuf::from(path).parent().unwrap().to_owned();
        std::fs::create_dir_all(&dir).ok();
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("curl -fSL '{url}' | bzip2 -d > '{path}'"))
            .status();
    }
}

fn main() {
    ensure_corpus();

    let args: Vec<String> = std::env::args().collect();
    let codec = args.get(1).map(|s| s.as_str()).unwrap_or("m77rip");
    let file = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or("corpus/silesia/dickens");
    let iters: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let level: u8 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);

    let data = std::fs::read(file).unwrap_or_else(|e| panic!("{file}: {e}"));

    eprintln!(
        "codec={codec} file={file} size={} level={level} iters={iters}",
        data.len(),
    );

    let bound = m77rip::compress_bound(data.len());
    let mut comp_buf = vec![0u8; bound];

    // Warmup
    for _ in 0..3 {
        match codec {
            "m77rip" => {
                let _ = m77rip::compress_into_level(
                    std::hint::black_box(&data),
                    std::hint::black_box(&mut comp_buf),
                    level,
                );
            }
            "cpp" => {
                c_compress_into(
                    std::hint::black_box(&data),
                    std::hint::black_box(&mut comp_buf),
                    level,
                );
            }
            _ => panic!("unknown codec: {codec}"),
        }
    }

    let start = cpu_nanos();
    for _ in 0..iters {
        match codec {
            "m77rip" => {
                let _ = m77rip::compress_into_level(
                    std::hint::black_box(&data),
                    std::hint::black_box(&mut comp_buf),
                    level,
                );
            }
            "cpp" => {
                c_compress_into(
                    std::hint::black_box(&data),
                    std::hint::black_box(&mut comp_buf),
                    level,
                );
            }
            _ => {}
        }
    }
    let elapsed = cpu_nanos() - start;
    let ns_per_op = elapsed as f64 / iters as f64;
    let mbps = data.len() as f64 / ns_per_op * 1000.0;
    eprintln!("{ns_per_op:.0} ns/op  {mbps:.0} MB/s");
}
