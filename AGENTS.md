# AGENTS.md

Rust encoder and decoder for the misa77 compression format. Benchmarked
against the C++ reference, vendored as the `vendor/misa77` git submodule.

## Workspace layout

- **`m77rip`** (root): facade. Re-exports core, encode, and decode.
  `build.rs` builds the C++ reference for the `c-reference` feature.
- **`m77rip-core`** (`crates/core`): wire format constants (`format.rs`) and
  `Error`.
- **`m77rip-encode`** (`crates/encode`): `compress*` and `compress_bound*`.
  Levels `-1..=3` write light streams, level `4` writes heavy streams.
  Levels `3` and `4` parse over a suffix array from `libsais`.
- **`m77rip-decode`** (`crates/decode`): `decompress`, `decompress_into`,
  `decompressed_size`. Supports `no_std` and 32-bit targets.
- **`examples/`**: `m77rip_bench` (Silesia benchmark, downloads the corpus),
  `profile_compress`, `profile_decompress` (`perf stat` harnesses).
- **`bench/`**: `m77rip_charts` SVG chart generator. Excluded from the
  workspace.
- **`fuzz/`**: `fuzz_roundtrip`, `fuzz_decomp_corrupt`, `fuzz_cpp_compress`
  (needs `c-reference`). Excluded from the workspace.

Encode and decode do not depend on each other. Separate crates keep their hot
loops in separate codegen units, so LTO register allocation in one does not
disturb the other.

## Cargo features

| feature | effect |
|---------|--------|
| `std` (default) | runtime SIMD detection, thread-local encoder workspaces, `std::error::Error` |
| `alloc` | `decompress` returning `Vec<u8>` |
| `paranoid` | no effect today; any future `unsafe` must stay out of builds with it |
| `c-reference` | builds the C++ reference (benchmarks, interop tests, `fuzz_cpp_compress`) |

Intra-workspace deps use `default-features = false` with explicit feature
forwarding.

## Format invariants

[`DESIGN.md`](DESIGN.md) describes the stream layout. The decoder relies on:

- Light tokens are 3 bytes (control + 16-bit distance) before any literal
  length extension bytes. Heavy tokens are 4 bytes.
- Minimum distance 33 exceeds the 32-byte light match maximum: light match
  copies never overlap their source.
- The raw literal suffix (at least 32 light, 64 heavy bytes) gives fixed-size
  copies room to run past the exact length.

## Build, test, lint

```sh
cargo test --workspace --lib --tests                         # default features
cargo test --workspace --lib --tests --features paranoid
cargo test --workspace --lib --tests --features c-reference  # C++ interop
cargo +nightly fmt --all -- --check                          # CI uses nightly rustfmt
cargo clippy --workspace --all-targets
cargo clippy --workspace --all-targets --all-features
```

**HARD RULE:** Clippy must pass under both configurations before pushing.
Fix warnings instead of silencing them. For a demonstrated false positive,
use the narrowest `#[expect(lint, reason = "...")]`. Use `#[allow]` only
when the lint fires in some feature combinations but not others.

Lints: clippy `pedantic` = warn, thresholds in `clippy.toml`.

Fuzzing without `cargo-fuzz`, plus Miri, sanitizers and charts: see
[`DEVELOPMENT.md`](DEVELOPMENT.md) and [`SAFETY.md`](SAFETY.md).

## Benchmarking and profiling

```sh
cargo run --release --example m77rip_bench --features c-reference
cargo run --release --example m77rip_bench --features c-reference,paranoid
cargo run --release --example profile_decompress --features c-reference -- m77rip dickens 2000 0
cargo run --release --example profile_decompress --features c-reference -- cpp dickens 2000 0
```

Best of 10 rounds at 20 ms each, first 1 MiB of each Silesia file. Results
are cached per codec in `~/.cache/m77rip/<arch>/<codec>.jsonl`. `--impl
m77rip` reruns only m77rip codecs (others come from the cache). `--files
dickens,xml` filters corpus files. The corpus lives in gitignored `corpus/`.

- Profile before optimizing. Change measured hotspots only. Back performance
  claims with before and after numbers from release builds.
- Fix build warnings, formatting, and clippy issues before benchmarking.
- Run benchmarks serially with no competing CPU load. Stop on warnings or
  timeouts and fix the cause before rerunning.

## Code quality

API design:

- Keep public APIs small and hard to misuse. Default to private or
  `pub(crate)`. Do not leak implementation types in public signatures.
- Take borrowed inputs (`&[u8]`, `&mut [u8]`) when ownership is not needed.
- Prefer standard traits (`From`/`TryFrom`, `Default`, `Display`). Add an
  inherent `new` or `to_*` only when no trait fits.
- Document every public item, with `# Errors` and `# Panics` where they
  apply. Make functions `const` where possible.

Code shape:

- One concern per module. Split a source file past about 800 lines (tests
  excluded) into submodules along its concerns. Unit tests go in a sibling
  `tests.rs`.
- Functions stay under 60 lines and 5 parameters (`clippy.toml`). When
  arguments travel together, pass a named struct. A flat `match` with one
  arm per level may `#[expect(clippy::too_many_lines)]`.
- Group struct fields that change together into a named sub-struct.
- Logic repeated across levels or formats lives in one shared helper, not in
  copies. Hot-loop specializations use generics or const generics, not
  copied bodies.
- Off the hot path, plain readable code beats clever code.

Safety and errors:

- All four crates are `#![forbid(unsafe_code)]`. Only tests, examples, and
  fuzz targets that call the C++ reference use `unsafe`, each block with a
  `// SAFETY:` comment.
- SIMD goes through `fearless_simd`. No raw intrinsics, no inline asm, no
  manual `#[target_feature]` dispatch.
- Hot loops avoid bounds checks by construction: fixed-size `[u8; N]` chunks
  (`first_chunk`, `try_into`), slices cut once per token, and headroom tests
  the compiler can see. Check codegen (`cargo asm`) when a check matters.
- Malformed input returns `Error`, never panics, never reads or writes out
  of bounds. Check lengths and limits before allocating.
- Encoder output stays byte-identical to the C++ reference at levels with
  exact-parity tests.

Tests:

- Every new or changed function is exercised by a test. Unit tests sit next
  to private logic; public behavior goes in `tests/`. Name tests by behavior.
- Add a regression test before fixing a bug.

## Conventions

- `main` is protected. All changes go through PRs. Never push to `main`.
- **HARD RULE: Never create documentation files unless explicitly asked.**
  Update existing files instead.
- **HARD RULE: No walls of text.** Keep docs brief and technical.
- `DESIGN.md` describes current behavior. No change history or benchmark
  numbers there.
- `CHANGELOG.md`: extend `## [Unreleased]` only. Never edit a released
  section.
- Releases go through the release-plz PR (see `DEVELOPMENT.md`). Do not
  publish to crates.io by hand.
- Rust 2024 edition, MSRV 1.89. ASCII-only source.
- `info/` is gitignored. It holds `M77-PERF.md` (misa77 optimization catalog
  and `perf stat` numbers) and `performance.md` (optimization journal).
