# Development

Run tests and lint:

```sh
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo +nightly fmt --all -- --check
```

Run fuzz targets without `cargo-fuzz`:

```sh
cargo run --manifest-path fuzz/Cargo.toml --bin fuzz_roundtrip -- -runs=10000
cargo run --manifest-path fuzz/Cargo.toml --bin fuzz_decomp_corrupt -- -runs=10000
cargo run --manifest-path fuzz/Cargo.toml --bin fuzz_cpp_compress -- -runs=10000
```

Benchmark only after tests, lint, and formatting pass:

```sh
cargo run --release --example m77rip_bench --features c-reference
cargo run --release --example m77rip_bench --features c-reference -- --impl m77rip --files dickens,xml
cargo run --release --example profile_decompress --features c-reference -- m77rip dickens 2000 0
```

The benchmark downloads the Silesia corpus into `corpus/silesia/` and caches
results per codec in `~/.cache/m77rip/<arch>/<codec>.jsonl`. `--impl` reruns
only matching codecs; the rest come from the cache.

Generate charts from cached benchmark results:

```sh
cargo run --manifest-path bench/Cargo.toml --bin m77rip_charts -- all \
  doc/charts/x86_64
```

The chart tool reads `~/.cache/m77rip/<arch>/*.jsonl` and requires
`@1MiB` rows for all 12 Silesia files. It writes SVGs with browser-friendly
`viewBox` sizing.

Use `.chart_hw` for local-only subtitle labels:

```text
prefix=local machine name
postfix=performance governor,turbo off
```
