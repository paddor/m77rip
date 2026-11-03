# Design

misa77 stores control tokens and literals in separate streams. The decoder
reads tokens forward and literals backward, then performs fixed-size copies.
The final 32 literal bytes provide safe padding for those copies.

The encoder keeps a 16-way hash table keyed by 4-byte sequences. It inserts
positions with a 32-byte lag, probes prior candidates, measures up to 32
matching bytes, then emits a token: literal length, match length, and 16-bit
distance. Literals are appended from the right side of the output buffer.

The decoder walks the token stream from byte 16 and the literal stream from
the suffix boundary. Each token copies literals to the current output cursor,
then copies a non-overlapping match from earlier output. Guarded prefix
and tail phases check every token. The middle phase, where no match can
reach before the output start, uses fixed 16-byte literal and 32-byte match
copies.

Encode and decode live in separate crates to keep hot-loop code generation
independent.
