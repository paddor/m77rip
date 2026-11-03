# Design

misa77 stores control tokens and literals in separate streams. The decoder
reads tokens forward and literals backward, then performs fixed-size copies.
The final 32 literal bytes provide safe padding for those copies.

The encoder keeps a 16-way hash table keyed by 4-byte sequences. It inserts
positions with a 32-byte lag, probes prior candidates, measures up to 32
matching bytes, then emits a token: literal length, match length, and 16-bit
distance. Literals are appended from the right side of the output buffer.

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
