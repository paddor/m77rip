# Safety

## No unsafe

All four crates (`m77rip`, `m77rip-core`, `m77rip-encode`, `m77rip-decode`)
are `#![forbid(unsafe_code)]`. No `unsafe` is exposed in the public API. SIMD
dispatch goes through `fearless_simd`, which keeps the `#[target_feature]`
calls inside that crate.

The decoder checks stream sizes, token bounds, output room, and match
distance before it copies. Malformed input returns `Error::CorruptInput`. It
never panics and never reads or writes outside the input and output slices.

Levels `3` and `4` build suffix arrays with C libsais through the safe API of
the `libsais` crate. The m77rip crates hold no FFI code.

The `paranoid` feature has no effect today. It stays so existing feature
lists keep building, and as a guarantee: if a later release adds `unsafe`
code for speed, builds with `paranoid` leave it out.

Tests, examples, and fuzz targets that call the vendored C++ reference use
`unsafe` for the FFI calls. They are not published.

## Checks

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test --workspace --features c-reference
cargo +nightly fuzz run fuzz_roundtrip -- -max_total_time=60
cargo +nightly fuzz run fuzz_decomp_corrupt -- -max_total_time=60
cargo +nightly fuzz run fuzz_cpp_compress -- -max_total_time=60
```
