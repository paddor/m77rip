/// Width of unconditional vector operations (bytes).
pub const VECTOR_WIDTH: usize = 32;

/// Inputs at or below this size are stored raw (no compression).
pub const SMALL_LIM: usize = 32;

/// Hash table insertion lag. Ensures minimum match distance >= HASHTAB_LAG + 1,
/// which guarantees match copies never overlap.
pub const HASHTAB_LAG: usize = 32;

/// Maximum match distance storable in a 16-bit offset field.
pub const DIS_LIM: usize = 65536;

/// Minimum match length the compressor will emit.
pub const MIN_MATCH_LEN: usize = 4;

/// Maximum match length per token (fits in 5-bit field + bias).
pub const MAX_MATCH_LEN: usize = 32;

/// Minimum raw suffix appended after all sequences.
pub const LITERAL_SUFFIX: usize = 32;

/// Number of bits in the token used for literal length.
pub const TOKEN_LIT_BITS: u32 = 3;

/// Number of bits in the token used for match length.
pub const TOKEN_MATCH_BITS: u32 = 5;

/// Maximum literal length encodable inline (before extension bytes).
pub const TOKEN_LIT_MAX: usize = (1 << TOKEN_LIT_BITS) - 1; // 7

/// Mask for the match length field in a token byte.
pub const TOKEN_MATCH_MASK: u8 = (1 << TOKEN_MATCH_BITS) - 1; // 0x1F

/// Size of the stream header: 8-byte original_size.
pub const HEADER_SIZE: usize = 8;

/// Size of the extended header (original_size + literal_suffix_cnt).
pub const EXT_HEADER_SIZE: usize = 16;

/// Minimum match distance.
pub const MIN_DISTANCE: usize = HASHTAB_LAG + 1; // 33

/// Maximum match distance.
pub const MAX_DISTANCE: usize = HASHTAB_LAG + DIS_LIM; // 65568
