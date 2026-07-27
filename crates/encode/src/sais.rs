const EMPTY: i32 = -1;
const PREFETCH_DISTANCE: usize = 64;
const PREFETCH_CURSOR_DISTANCE: usize = 5;

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
use core::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
#[inline(always)]
fn prefetch<T>(slice: &[T], index: usize) {
    if index < slice.len() {
        // SAFETY: guarded by the bounds check. Prefetch only hints cache.
        unsafe { _mm_prefetch(slice.as_ptr().add(index).cast::<i8>(), _MM_HINT_T0) };
    }
}

#[cfg(any(feature = "paranoid", not(target_arch = "x86_64")))]
#[inline(always)]
fn prefetch<T>(_slice: &[T], _index: usize) {}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn at<T>(slice: &[T], index: usize) -> &T {
    debug_assert!(index < slice.len());
    // SAFETY: SAIS maintains all bucket, suffix-array, and type-array indices
    // inside the slices sized for the current recursion level.
    unsafe { slice.get_unchecked(index) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn at<T>(slice: &[T], index: usize) -> &T {
    &slice[index]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn at_mut<T>(slice: &mut [T], index: usize) -> &mut T {
    debug_assert!(index < slice.len());
    // SAFETY: same invariant as `at`; mutable callers never alias one slot.
    unsafe { slice.get_unchecked_mut(index) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
fn at_mut<T>(slice: &mut [T], index: usize) -> &mut T {
    &mut slice[index]
}

pub(crate) struct SaisWorkspace {
    s32: Vec<i32>,
    st_pool: Vec<u32>,
    cnt_pool: Vec<usize>,
    lms_pool: Vec<usize>,
    names: Vec<i32>,
    bkt: Vec<usize>,
}

impl SaisWorkspace {
    pub(crate) fn new() -> Self {
        Self {
            s32: Vec::new(),
            st_pool: Vec::new(),
            cnt_pool: Vec::new(),
            lms_pool: Vec::new(),
            names: Vec::new(),
            bkt: Vec::new(),
        }
    }

    pub(crate) fn suffix_array_with_rank(
        &mut self,
        input: &[u8],
        sa: &mut [i32],
        rank: &mut [i32],
    ) {
        debug_assert_eq!(input.len(), sa.len());
        debug_assert_eq!(input.len(), rank.len());
        if input.is_empty() {
            return;
        }

        let mut symbols = core::mem::take(&mut self.s32);
        symbols.resize(input.len(), 0);
        let mut min = input[0];
        let mut max = input[0];
        for &byte in input {
            min = min.min(byte);
            max = max.max(byte);
        }
        for (dst, &byte) in symbols.iter_mut().zip(input) {
            *dst = i32::from(byte - min);
        }

        let k = usize::from(max - min) + 1;
        self.core(&symbols, sa, k, 0, 0, 0);
        self.s32 = symbols;

        for (i, &pos) in sa.iter().enumerate() {
            if i + PREFETCH_DISTANCE < sa.len() {
                prefetch(rank, sa[i + PREFETCH_DISTANCE] as usize);
            }
            debug_assert!(pos >= 0);
            rank[pos as usize] = i as i32;
        }
    }

    fn core(
        &mut self,
        s: &[i32],
        sa: &mut [i32],
        k: usize,
        st_off: usize,
        cnt_off: usize,
        lms_off: usize,
    ) {
        let n = s.len();
        if n == 0 {
            return;
        }
        if n == 1 {
            sa[0] = 0;
            return;
        }

        self.ensure_space(n, k, st_off, cnt_off, lms_off);

        let mut lms_count = 0usize;
        {
            let st = &mut self.st_pool[st_off..st_off + n];
            let cnt = &mut self.cnt_pool[cnt_off..cnt_off + k + 1];
            let lms = &mut self.lms_pool[lms_off..lms_off + n / 2 + 1];

            cnt.fill(0);
            let last_symbol = s[n - 1] as usize;
            *at_mut(st, n - 1) = (last_symbol as u32) << 1;
            *at_mut(cnt, last_symbol) += 1;

            let mut next_type = 0u32;
            for i in (0..n - 1).rev() {
                let symbol = s[i] as usize;
                *at_mut(cnt, symbol) += 1;

                let this_type = if s[i] < s[i + 1] {
                    1
                } else if s[i] > s[i + 1] {
                    0
                } else {
                    next_type
                };
                *at_mut(st, i) = ((symbol as u32) << 1) | this_type;

                if next_type == 1 && this_type == 0 {
                    *at_mut(lms, lms_count) = i + 1;
                    lms_count += 1;
                }
                next_type = this_type;
            }
        }

        {
            let cnt = &mut self.cnt_pool[cnt_off..cnt_off + k + 1];
            let mut acc = 0usize;
            for count in cnt.iter_mut() {
                let value = *count;
                *count = acc;
                acc += value;
            }
        }

        sa.fill(EMPTY);
        if lms_count != 0 {
            self.copy_cursors(k, cnt_off);
            let st = &self.st_pool[st_off..st_off + n];
            let lms = &self.lms_pool[lms_off..lms_off + n / 2 + 1];
            for &idx in lms.iter().take(lms_count) {
                let bucket = ((*at(st, idx) >> 1) as usize) + 1;
                let slot = *at(&self.bkt, bucket) - 1;
                *at_mut(&mut self.bkt, bucket) = slot;
                *at_mut(sa, slot) = idx as i32;
            }
        }
        self.induce(s, sa, k, st_off, cnt_off);

        if lms_count == 0 {
            return;
        }

        let mut write = 0usize;
        let st = &self.st_pool[st_off..st_off + n];
        let mut i = 0usize;
        while i < n {
            let pos = *at(sa, i);
            if i + PREFETCH_DISTANCE < n {
                let ahead = *at(sa, i + PREFETCH_DISTANCE);
                if ahead > 0 {
                    prefetch(st, ahead as usize - 1);
                }
            }
            if pos > 0 {
                let pos = pos as usize;
                if is_lms(st, pos) {
                    *at_mut(sa, write) = pos as i32;
                    write += 1;
                }
            }
            i += 1;
        }
        debug_assert_eq!(write, lms_count);

        self.names
            .resize(self.names.len().max(n.div_ceil(2)), EMPTY);
        let mut prev = sa[0] as usize;
        let mut label = 1usize;
        self.names[prev >> 1] = 0;
        for &cur in sa.iter().take(lms_count).skip(1) {
            let cur = cur as usize;
            if !lms_equal(st, prev, cur, n) {
                label += 1;
            }
            self.names[cur >> 1] = (label - 1) as i32;
            prev = cur;
        }

        let mut reduced = vec![0i32; lms_count];
        let lms = &self.lms_pool[lms_off..lms_off + n / 2 + 1];
        for (i, slot) in reduced.iter_mut().enumerate() {
            let pos = *at(lms, lms_count - 1 - i);
            *slot = *at(&self.names, pos >> 1);
        }

        if label < lms_count {
            self.core(
                &reduced,
                &mut sa[..lms_count],
                label,
                st_off + n,
                cnt_off + k + 1,
                lms_off + n / 2 + 1,
            );
        } else {
            for i in 0..lms_count {
                sa[reduced[i] as usize] = i as i32;
            }
        }

        sa[lms_count..].fill(EMPTY);
        self.copy_cursors(k, cnt_off);
        let st = &self.st_pool[st_off..st_off + n];
        let lms = &self.lms_pool[lms_off..lms_off + n / 2 + 1];
        for i in (0..lms_count).rev() {
            let order = *at(sa, i) as usize;
            let pos = *at(lms, lms_count - 1 - order);
            *at_mut(sa, i) = EMPTY;
            let bucket = ((*at(st, pos) >> 1) as usize) + 1;
            let slot = *at(&self.bkt, bucket) - 1;
            *at_mut(&mut self.bkt, bucket) = slot;
            *at_mut(sa, slot) = pos as i32;
        }
        self.induce(s, sa, k, st_off, cnt_off);
    }

    fn ensure_space(&mut self, n: usize, k: usize, st_off: usize, cnt_off: usize, lms_off: usize) {
        if self.st_pool.len() < st_off + n {
            self.st_pool.resize(st_off + n, 0);
        }
        if self.cnt_pool.len() < cnt_off + k + 1 {
            self.cnt_pool.resize(cnt_off + k + 1, 0);
        }
        if self.lms_pool.len() < lms_off + n / 2 + 1 {
            self.lms_pool.resize(lms_off + n / 2 + 1, 0);
        }
        if self.bkt.len() < k + 1 {
            self.bkt.resize(k + 1, 0);
        }
        if self.names.len() < n.div_ceil(2) {
            self.names.resize(n.div_ceil(2), EMPTY);
        }
    }

    fn copy_cursors(&mut self, k: usize, cnt_off: usize) {
        self.bkt[..=k].copy_from_slice(&self.cnt_pool[cnt_off..cnt_off + k + 1]);
    }

    fn induce(&mut self, s: &[i32], sa: &mut [i32], k: usize, st_off: usize, cnt_off: usize) {
        let n = s.len();
        self.copy_cursors(k, cnt_off);
        {
            let st = &self.st_pool[st_off..st_off + n];
            let bkt = &mut self.bkt;
            let last_bucket = (*at(st, n - 1) >> 1) as usize;
            let slot = *at(bkt, last_bucket);
            *at_mut(sa, slot) = (n - 1) as i32;
            *at_mut(bkt, last_bucket) = slot + 1;

            for i in 0..n {
                if i + PREFETCH_DISTANCE < n {
                    let ahead = *at(sa, i + PREFETCH_DISTANCE);
                    if ahead > 0 {
                        prefetch(st, ahead as usize - 1);
                    }
                }
                if i + PREFETCH_CURSOR_DISTANCE < n {
                    let ahead = *at(sa, i + PREFETCH_CURSOR_DISTANCE);
                    let prev = if ahead > 0 { ahead as usize - 1 } else { 0 };
                    prefetch(bkt, (*at(st, prev) >> 1) as usize);
                }
                let j = *at(sa, i);
                if j > 0 {
                    let prev = (j - 1) as usize;
                    let val = *at(st, prev);
                    if val & 1 == 0 {
                        let bucket = (val >> 1) as usize;
                        let slot = *at(bkt, bucket);
                        *at_mut(sa, slot) = prev as i32;
                        *at_mut(bkt, bucket) = slot + 1;
                    }
                }
            }
        }

        self.copy_cursors(k, cnt_off);
        {
            let st = &self.st_pool[st_off..st_off + n];
            let bkt = &mut self.bkt;
            for i in (0..n).rev() {
                if i >= PREFETCH_DISTANCE {
                    let ahead = *at(sa, i - PREFETCH_DISTANCE);
                    if ahead > 0 {
                        prefetch(st, ahead as usize - 1);
                    }
                }
                if i >= PREFETCH_CURSOR_DISTANCE {
                    let ahead = *at(sa, i - PREFETCH_CURSOR_DISTANCE);
                    let prev = if ahead > 0 { ahead as usize - 1 } else { 0 };
                    prefetch(bkt, ((*at(st, prev) >> 1) as usize) + 1);
                }
                let j = *at(sa, i);
                if j > 0 {
                    let prev = (j - 1) as usize;
                    let val = *at(st, prev);
                    if val & 1 != 0 {
                        let bucket = ((val >> 1) as usize) + 1;
                        let slot = *at(bkt, bucket) - 1;
                        *at_mut(bkt, bucket) = slot;
                        *at_mut(sa, slot) = prev as i32;
                    }
                }
            }
        }
    }
}

fn is_lms(st: &[u32], pos: usize) -> bool {
    pos > 0 && *at(st, pos) & 1 != 0 && *at(st, pos - 1) & 1 == 0
}

fn lms_equal(st: &[u32], prev: usize, cur: usize, n: usize) -> bool {
    for off in 0.. {
        let p = prev + off;
        let q = cur + off;
        if p == n || q == n || at(st, p) != at(st, q) {
            return false;
        }
        if off > 0 {
            let p_end = is_lms(st, p);
            let q_end = is_lms(st, q);
            if p_end && q_end {
                return true;
            }
            if p_end != q_end {
                return false;
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suffix_array(input: &[u8]) -> Vec<i32> {
        let mut workspace = SaisWorkspace::new();
        let mut sa = vec![0; input.len()];
        let mut rank = vec![0; input.len()];
        workspace.suffix_array_with_rank(input, &mut sa, &mut rank);
        for (i, &pos) in sa.iter().enumerate() {
            assert_eq!(rank[pos as usize], i as i32);
        }
        sa
    }

    fn naive_suffix_array(input: &[u8]) -> Vec<i32> {
        let mut sa: Vec<_> = (0..input.len()).collect();
        sa.sort_by(|&a, &b| input[a..].cmp(&input[b..]));
        sa.into_iter().map(|pos| pos as i32).collect()
    }

    #[test]
    fn sorts_banana() {
        assert_eq!(suffix_array(b"banana"), vec![5, 3, 1, 0, 4, 2]);
    }

    #[test]
    fn matches_naive_for_small_patterns() {
        let cases: &[&[u8]] = &[
            b"",
            b"a",
            b"aaaaaa",
            b"abababab",
            b"mississippi",
            b"the quick brown fox jumps over the lazy dog",
            &[3, 1, 4, 1, 5, 9, 2, 6, 5],
        ];
        for &case in cases {
            assert_eq!(suffix_array(case), naive_suffix_array(case), "{case:?}");
        }
    }
}
