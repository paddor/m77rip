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

### Pipeline summary

![Summary](https://raw.githubusercontent.com/paddor/m77rip/main/doc/charts/x86_64/summary.svg)

Stacked bars show encode + transfer at 100 MB/s + decode for levels `-1`, `0`,
`1`, and `2`. Each panel aggregates compressible and incompressible Silesia
inputs. Lower is better. Benchmarks use the first 1 MiB from each Silesia
file, single-threaded on x86_64 (AVX2). Best of 10 rounds at 20 ms each.

### Default build

Tables report geomeans across files in each corpus class. Values in
parentheses compare against C++ misa77. Encode/decode parentheses are speed
ratios. Compression-ratio parentheses are ratio-vs-ratio values, so higher is
better. Level `-1` has no C++ misa77 peer, so it compares against C++ level 0.

| Level | Corpus | Encode MB/s (vs misa77) | Decode MB/s (vs misa77) | Ratio (vs misa77) |
|-------|--------|--------------------------|--------------------------|-------------------|
| `L-1` | Compressible | 384 (5.97x) | 3639 (0.64x) | 1.63 (0.64x) |
| `L-1` | Incompressible | 618 (14.43x) | 4488 (0.68x) | 1.18 (0.88x) |
| `L0` | Compressible | 122 (1.90x) | 5188 (0.91x) | 2.49 (0.98x) |
| `L0` | Incompressible | 89.9 (2.10x) | 5939 (0.90x) | 1.27 (0.94x) |
| `L1` | Compressible | 84.6 (1.33x) | 5545 (1.10x) | 2.64 (0.98x) |
| `L1` | Incompressible | 40.0 (1.33x) | 3114 (1.16x) | 1.49 (0.99x) |
| `L2` | Compressible | 20.4 (3.48x) | 4781 (0.90x) | 2.82 (0.97x) |
| `L2` | Incompressible | 18.3 (2.79x) | 3094 (0.91x) | 1.50 (0.99x) |

### Paranoid build (`--features paranoid`, zero unsafe)

| Level | Corpus | Encode MB/s (vs misa77) | Decode MB/s (vs misa77) | Ratio (vs misa77) |
|-------|--------|--------------------------|--------------------------|-------------------|
| `L-1` | Compressible | 252 (3.91x) | 1309 (0.23x) | 1.66 (0.65x) |
| `L-1` | Incompressible | 241 (5.64x) | 1410 (0.21x) | 1.20 (0.89x) |
| `L0` | Compressible | 71.9 (1.12x) | 1724 (0.30x) | 2.49 (0.98x) |
| `L0` | Incompressible | 60.1 (1.40x) | 2574 (0.39x) | 1.27 (0.94x) |
| `L1` | Compressible | 45.3 (0.71x) | 1842 (0.36x) | 2.64 (0.98x) |
| `L1` | Incompressible | 22.9 (0.77x) | 1095 (0.41x) | 1.49 (0.99x) |
| `L2` | Compressible | 19.5 (3.32x) | 1296 (0.24x) | 2.82 (0.97x) |
| `L2` | Incompressible | 17.1 (2.60x) | 857 (0.25x) | 1.50 (0.99x) |

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

`compress_level` accepts levels `-1`, `0`, `1`, and `2`. Level `-1` is
fastest, level `0` is the C++-ratio middle ground, level `1` favors ratio,
and level `2` writes misa77 v0.5 heavy-format streams.

## License

MIT
