# m77rip

Fast, memory-safe Rust encoder and decoder for the
[misa77](https://github.com/welcome-to-the-sunny-side/misa77) compression
format.

misa77 is an LZ-based codec optimized for write-once, read-many workloads.
It trades compression speed for extremely fast single-threaded decompression
(1.5-2x faster than LZ4) via a split-stream layout that separates control
tokens from literal data.

## Usage

```rust
use m77rip::{compress, decompress};

let input = b"the quick brown fox jumps over the lazy dog, again and again!";
let compressed = compress(input);
let decompressed = decompress(&compressed, input.len()).unwrap();
assert_eq!(&decompressed, input);
```

For zero-allocation decompression into a caller-provided buffer:

```rust
use m77rip::{compress, decompress_into, decompressed_size};

let compressed = compress(b"hello world, hello world!");
let size = decompressed_size(&compressed).unwrap() as usize;
let mut buf = vec![0u8; size];
let written = decompress_into(&compressed, &mut buf).unwrap();
assert_eq!(written, size);
```

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `std` | yes | Standard library. Enables runtime SIMD detection and `std::error::Error` impl. |
| `alloc` | yes (via `std`) | Enables `decompress()` which returns a `Vec<u8>`. |
| `paranoid` | no | Zero-unsafe build. All primitives use bounds-checked operations. Output is byte-identical. |
| `c-reference` | no | Builds the vendored C++ reference implementation for benchmarking. |

## Performance

### Pipeline charts

![Summary](https://raw.githubusercontent.com/paddor/m77rip/main/doc/charts/x86_64/summary.svg)
![Per-file pipeline](https://raw.githubusercontent.com/paddor/m77rip/main/doc/charts/x86_64/pipeline.svg)

Stacked bars show level 0 encode + transfer at 1 GB/s + decode. Lower is
better. Benchmarks use misa77 v0.3.0, Silesia corpus, single-threaded on
x86_64 (AVX2). Best of 10 rounds at 20 ms each.

### Default build

| File    | Decode -0 | Decode -1 | Encode -0 | Encode -1 |
|---------|-----------|-----------|-----------|-----------|
| dickens |     1.01x |     1.02x |     1.03x |     1.05x |
| mozilla |     1.01x |     1.02x |     1.00x |     0.98x |
| mr      |     1.04x |     1.02x |     1.04x |     1.09x |
| nci     |     1.04x |     1.03x |     1.00x |     0.97x |
| ooffice |     1.00x |     0.99x |     1.00x |     0.97x |
| osdb    |     0.93x |     0.94x |     1.00x |     0.96x |
| reymont |     1.01x |     1.01x |     0.99x |     1.03x |
| samba   |     1.01x |     1.00x |     1.00x |     0.98x |
| sao     |     0.95x |     0.94x |     1.01x |     0.99x |
| webster |     1.02x |     1.03x |     1.01x |     0.99x |
| x-ray   |     1.01x |     0.98x |     0.99x |     0.96x |
| xml     |     1.00x |     1.00x |     1.02x |     1.01x |
| **geomean** | **1.00x** | **1.00x** | **1.01x** | **1.00x** |

### Paranoid build (`--features paranoid`, zero unsafe)

| File    | Decode -0 | Decode -1 | Encode -0 | Encode -1 |
|---------|-----------|-----------|-----------|-----------|
| dickens |     0.29x |     0.28x |     0.63x |     0.63x |
| mozilla |     0.38x |     0.38x |     0.56x |     0.53x |
| mr      |     0.40x |     0.38x |     0.60x |     0.63x |
| nci     |     0.39x |     0.38x |     0.43x |     0.41x |
| ooffice |     0.41x |     0.38x |     0.58x |     0.56x |
| osdb    |     0.34x |     0.34x |     0.58x |     0.56x |
| reymont |     0.30x |     0.29x |     0.54x |     0.52x |
| samba   |     0.35x |     0.33x |     0.54x |     0.52x |
| sao     |     0.34x |     0.41x |     0.59x |     0.58x |
| webster |     0.30x |     0.30x |     0.58x |     0.57x |
| x-ray   |     0.73x |     0.47x |     0.67x |     0.63x |
| xml     |     0.29x |     0.29x |     0.50x |     0.49x |
| **geomean** | **0.36x** | **0.35x** | **0.56x** | **0.55x** |

## `no_std` and 32-bit support

The `m77rip-decode` crate supports `no_std` and 32-bit targets. Depend on
it directly with default features disabled:

```toml
[dependencies]
m77rip-decode = { version = "0.1", default-features = false }
```

This gives you `decompress_into` and `decompressed_size`. Add the `alloc`
feature to also get `decompress`.

Without `std`, SIMD dispatch uses compile-time target feature detection
instead of runtime cpuid. Compile with `-C target-cpu=native` or
`-C target-feature=+avx2` to enable AVX2 on x86_64.

The encoder requires `std` and has only been tested on 64-bit targets.

`compress_level` returns an error for levels other than `0` and `1`.

## License

MIT
