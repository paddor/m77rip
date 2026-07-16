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

Throughput relative to the C++ reference implementation (misa77 v0.2.0),
Silesia corpus, single-threaded on x86_64 (AVX2). Best of 10 rounds at
20 ms each.

### Default build

| File    | Decode -0 | Decode -1 | Encode -0 | Encode -1 |
|---------|-----------|-----------|-----------|-----------|
| dickens |     0.59x |     0.58x |     0.68x |     0.75x |
| mozilla |     0.73x |     0.70x |     0.63x |     0.64x |
| mr      |     0.65x |     0.73x |     0.65x |     0.66x |
| nci     |     0.71x |     0.83x |     0.56x |     0.62x |
| ooffice |     0.80x |     0.71x |     0.64x |     0.80x |
| osdb    |     0.64x |     0.71x |     0.65x |     0.65x |
| reymont |     0.58x |     0.57x |     0.65x |     0.70x |
| samba   |     0.66x |     0.65x |     0.63x |     0.65x |
| sao     |     0.62x |     0.70x |     0.63x |     0.64x |
| webster |     0.65x |     0.63x |     0.65x |     0.70x |
| x-ray   |     0.93x |     0.78x |     0.65x |     0.63x |
| xml     |     0.59x |     0.59x |     0.61x |     0.63x |
| **geomean** | **0.67x** | **0.68x** | **0.64x** | **0.67x** |

### Paranoid build (`--features paranoid`, zero unsafe)

| File    | Decode -0 | Decode -1 | Encode -0 | Encode -1 |
|---------|-----------|-----------|-----------|-----------|
| dickens |     0.24x |     0.23x |     0.60x |     0.60x |
| mozilla |     0.32x |     0.31x |     0.55x |     0.52x |
| mr      |     0.29x |     0.31x |     0.57x |     0.58x |
| nci     |     0.31x |     0.35x |     0.42x |     0.43x |
| ooffice |     0.37x |     0.33x |     0.57x |     0.68x |
| osdb    |     0.29x |     0.32x |     0.55x |     0.54x |
| reymont |     0.22x |     0.22x |     0.54x |     0.52x |
| samba   |     0.28x |     0.26x |     0.52x |     0.50x |
| sao     |     0.28x |     0.35x |     0.56x |     0.55x |
| webster |     0.26x |     0.25x |     0.55x |     0.54x |
| x-ray   |     0.66x |     0.41x |     0.59x |     0.55x |
| xml     |     0.23x |     0.23x |     0.47x |     0.46x |
| **geomean** | **0.30x** | **0.29x** | **0.54x** | **0.54x** |

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
