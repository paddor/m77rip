# Design

misa77 stores control tokens and literals in separate streams. The decoder
reads tokens forward and literals backward, then performs fixed-size copies.
The final 32 literal bytes provide safe padding for those copies.

Levels `-1` to `2` share one greedy loop. It probes a hash table of earlier
positions (the latest per 6-byte hash at `-1` and `0`, a ring of 16 or 20 per
4-byte hash at `1` and `2`), defers short matches as candidates, and emits
when a candidate fires. Positions enter the tables 32 bytes late, so every
match lies at least 33 bytes back. Levels `3` and `4` take the longest match
among each position's suffix-array neighbors inside the distance window.
Level `3` parses lazily; level `4` runs an exact shortest-path parse over
token byte costs. Writers append tokens forward and literal runs backward
from the end of the output, then join them.

The decoder walks the token stream from byte 16 and the literal stream from
the suffix boundary. Each token copies literals to the output cursor, then
copies a match from earlier output in 32-byte chunks. Chunks may run past
the exact length; the raw suffix gives them room.

The decoder is safe Rust. It decodes tokens in groups of four. A group
borrows fixed-size arrays of its control bytes, its literal bytes, and its
output, so offsets bounded by the token field widths need no bounds checks.
Light groups past the first 64 KiB borrow the whole match history in one
array; earlier groups check each distance. Heavy distances reach 1 MiB, so
each heavy token borrows its own window from the match source. Tokens a
group cannot take, such as long literal runs, and tokens near the stream
end go through a fully checked step, as do light outputs under 64 KiB.

Encode and decode live in separate crates to keep hot-loop code generation
independent.
