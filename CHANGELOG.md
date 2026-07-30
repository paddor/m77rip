# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

- Updated compression levels to the misa77 v0.6.0 model: integer levels
  `-1..=4`, level `1` default, light streams for `-1..=3`, and heavy streams
  for `4`.
- Ported the v0.6 light encoders for blitz, swift, loose, keen, and optimal
  parsing.
- Optimized default level `3` encoding with an AVX2 match finder and a lazy
  suffix-array parser.
- Optimized paranoid level `3` encoding with the same safe lazy suffix-array
  parser.
- Updated benchmark and profiling tools to accept signed compression levels.

## [0.1.6] - 2026-07-29

- Corrected the paranoid level `2` README benchmark rows after refreshing the
  stale cache entries from before the misa77 v0.5 encoder port.
- Optimized paranoid level `2` suffix-array construction and workspace reuse
  while preserving byte-identical output.
- Optimized paranoid level `1` compression with safe SIMD match probing.
- Clarified paranoid safety scope: the m77rip encode/decode crates forbid
  unsafe code, while level `2` suffix sorting uses the safe `libsais` API.

## [0.1.5] - 2026-07-28

- Ported level `2` compression to the misa77 v0.5 suffix-array matcher and
  exact block-DP parser.
- Updated the vendored C++ reference to misa77 v0.5.0 and added level `2`
  byte-parity coverage against it.
- Refreshed README performance tables and the summary chart from current
  1 MiB Silesia benchmark cache.

## [0.1.4] - 2026-07-23

- Optimized misa77-compatible encoder hot paths without changing encoded sizes
  for the benchmarked level 0, 1, and 2 streams.
- Improved default-build 1 MiB Silesia encoder geomeans to about 113 MB/s at
  level 0, 74 MB/s at level 1, and 21 MB/s at level 2.
- Improved paranoid-build 1 MiB Silesia encoder geomeans to about 70 MB/s at
  level 0, 42 MB/s at level 1, and 20 MB/s at level 2.
- Kept unsafe encapsulation unchanged; the speedups come from safe loop-shape
  changes, bounded heavy-chain insertion, and parser control-flow cleanup.
- Refreshed the benchmark summary chart from a fresh default and paranoid
  1 MiB Silesia run.

## [0.1.3] - 2026-07-22

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
- Condensed README performance tables to geomeans by level and corpus class,
  with speed and ratio comparisons against C++ misa77.
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
