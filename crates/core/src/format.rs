/// Width of unconditional vector operations (bytes).
pub const VECTOR_WIDTH: usize = 32;

/// Low 56 bits of the stream header store decompressed size.
pub const SIZE_MASK: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// High header byte stores format flags.
pub const FLAG_SHIFT: u32 = 56;

/// Header flag bit for the heavy format.
pub const FLAG_HEAVY: u8 = 1 << 0;

/// Header flag bit hinting that long heavy matches are rare.
pub const FLAG_HEAVY_COND: u8 = 1 << 1;

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

/// Heavy-format raw threshold.
pub const HEAVY_SMALL_LIM: usize = 64;

/// Heavy-format minimum raw suffix.
pub const HEAVY_LITERAL_SUFFIX: usize = 64;

/// Heavy-format literal length inline limit.
pub const HEAVY_TOKEN_LIT_MAX: usize = 63;

/// Heavy-format distance window bits.
pub const HEAVY_WIN_BITS: u32 = 20;

/// Heavy-format distance count.
pub const HEAVY_NDIS: usize = 1 << HEAVY_WIN_BITS;

/// Heavy-format minimum match distance.
pub const HEAVY_MIN_DISTANCE: usize = HASHTAB_LAG + 1;

/// Heavy-format maximum match distance.
pub const HEAVY_MAX_DISTANCE: usize = HEAVY_MIN_DISTANCE + HEAVY_NDIS - 1;

/// Heavy-format maximum match length.
pub const HEAVY_MAX_MATCH_LEN: usize = 192;

/// Heavy-format 20-bit distance mask.
pub const HEAVY_DIS_MASK: u32 = (1 << HEAVY_WIN_BITS) - 1;

/// Heavy-format match length code -> decoded length.
pub const HEAVY_LEN_OF: [u8; 64] = [
    0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64, 68, 72, 76,
    80, 84, 88, 92, 96, 100, 104, 108, 112, 116, 120, 124, 128, 160, 192,
];

/// Heavy-format decoded length -> largest encodable length not greater than it.
pub const HEAVY_LEN_FLOOR: [u8; 256] = heavy_len_floor_table();

/// Heavy-format decoded length -> code for [`HEAVY_LEN_FLOOR`].
pub const HEAVY_CODE_FLOOR: [u8; 256] = heavy_code_floor_table();

const fn heavy_len_floor_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut len = 0usize;
    while len < 256 {
        let mut best = 0u8;
        let mut c = 0usize;
        while c < HEAVY_LEN_OF.len() {
            let candidate = HEAVY_LEN_OF[c];
            if candidate as usize <= len && candidate >= best {
                best = candidate;
            }
            c += 1;
        }
        table[len] = best;
        len += 1;
    }
    table
}

const fn heavy_code_floor_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut len = 0usize;
    while len < 256 {
        let mut best_len = 0u8;
        let mut best_code = 0u8;
        let mut c = 0usize;
        while c < HEAVY_LEN_OF.len() {
            let candidate = HEAVY_LEN_OF[c];
            if candidate as usize <= len && candidate >= best_len {
                best_len = candidate;
                best_code = c as u8;
            }
            c += 1;
        }
        table[len] = best_code;
        len += 1;
    }
    table
}
