use super::*;

#[test]
fn heavy_len_floor_tables_round_down_to_encodable_lengths() {
    for len in 0..256 {
        let floor = HEAVY_LEN_FLOOR[len] as usize;
        let code = HEAVY_CODE_FLOOR[len] as usize;
        assert!(floor <= len);
        assert_eq!(HEAVY_LEN_OF[code] as usize, floor);
        if len >= MIN_MATCH_LEN {
            assert!(floor >= MIN_MATCH_LEN);
        }
    }
}

#[test]
fn heavy_token_fields_stay_in_range() {
    for token in [0u32, u32::MAX, 0xFC00_0000, 0x03F0_0000, HEAVY_DIS_MASK] {
        let lit_len = (token >> 26) as usize;
        let match_code = ((token >> 20) & 0x3F) as usize;
        let dis = (token & HEAVY_DIS_MASK) as usize + HEAVY_MIN_DISTANCE;
        assert!(lit_len <= HEAVY_TOKEN_LIT_MAX);
        assert!(match_code < HEAVY_LEN_OF.len());
        assert!((HEAVY_MIN_DISTANCE..=HEAVY_MAX_DISTANCE).contains(&dis));
    }
}
