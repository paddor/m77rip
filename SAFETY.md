# Safety

Safe public APIs validate stream sizes, token bounds, output capacity, match
distance, and fixed-copy padding before calling unchecked decoder primitives.
Unsafe memory operations live in `crates/decode/src/primitives.rs` and the
default encoder SIMD/load helpers in `crates/encode/src/encode.rs`.

The `paranoid` feature replaces those primitives with bounds-checked operations
and enables `#![forbid(unsafe_code)]` in encode and decode crates. Both builds
must produce compatible streams and decoded output.

Check the unsafe path with:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --features c-reference -- -D warnings
cargo clippy --workspace --all-targets --features c-reference,paranoid -- -D warnings
cargo test --workspace --features c-reference
cargo test --workspace --features c-reference,paranoid
cargo +nightly miri test --workspace --lib --tests
cargo kani --workspace
cargo +nightly fuzz run fuzz_roundtrip -- -max_total_time=60
cargo +nightly fuzz run fuzz_decomp_corrupt -- -max_total_time=60
cargo +nightly fuzz run fuzz_cpp_compress -- -max_total_time=60
ASAN_OPTIONS=detect_leaks=1:halt_on_error=1 \
RUSTFLAGS='-Zsanitizer=address' \
cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu \
  --workspace --features c-reference --tests
```
