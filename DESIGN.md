# Design

misa77 stores control tokens and literals in separate streams. The decoder
reads tokens forward and literals backward, then performs fixed-size copies.
The final 32 literal bytes provide safe padding for those copies.

Encode and decode live in separate crates to keep hot-loop code generation
independent. `paranoid` swaps unchecked primitives for safe twins without
changing the decoder interface.
