# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

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
