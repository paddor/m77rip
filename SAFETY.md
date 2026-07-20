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
cargo +nightly miri test --workspace --lib --tests
cargo test --workspace --all-targets --features paranoid
```
