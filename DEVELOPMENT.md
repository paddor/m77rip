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

Benchmark only after tests, lint, and formatting pass. See `CLAUDE.md` for
benchmark commands and corpus details.
