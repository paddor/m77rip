# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

- Added explicit level `-1` speed-first encoding while keeping misa77-compatible
  streams.
- Added misa77 v0.4 level 2 heavy-format encode/decode compatibility.
- Added level-specific compression bounds through `compress_bound_level`.
- Reworked level 0, 1, and 2 parser constants to recover ratio while hitting
  the current default-build encoder speed targets.
- Updated the benchmark summary chart to show level `-1`, `0`, `1`, and `2`
  aggregate pipelines across compressible and incompressible inputs.
- Set the benchmark summary transfer segment to 100 MB/s.
- Split level 2 into a taller side panel so slower heavy encoding does not
  dominate the other level panels.
- Kept the default encoder unsafe surface unchanged and covered the current
  release candidate with fmt, clippy, tests, Miri, Kani, ASan, and fuzz smoke
  runs.

## [0.1.2] - 2026-07-19

- Reduced the published root crate package to runtime/library files and the
  vendored C++ source required by the optional `c-reference` feature.
- Fixed README chart image URLs to include the `main` branch segment.

## [0.1.1] - 2026-07-19

- Optimized default decoder and encoder hot paths to match misa77 v0.3.0
  throughput on the Silesia benchmark set.
- Added Kani proofs for decoder fast-loop bounds, encoder hash-table recovery,
  and SIMD vector-load preconditions.
- Added corrupt-input regression coverage and expanded roundtrip/fuzz coverage.
- Added benchmark chart generation and README performance charts.

## [0.1.0] - 2026-07-17

- Initial decoder implementation for the misa77 stream format.
- Simple reference encoder for testing.
