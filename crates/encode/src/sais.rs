#[cfg(not(feature = "paranoid"))]
const PREFETCH_DISTANCE: usize = 80;

#[cfg(all(not(feature = "paranoid"), target_arch = "x86_64"))]
use core::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};

use libsais::{SuffixArrayConstruction, context::Context, typestate::SingleThreaded};

#[cfg(all(not(feature = "paranoid"), not(target_arch = "x86_64")))]
#[inline(always)]
fn prefetch<T>(_slice: &[T], _index: usize) {}

pub(crate) struct SaisWorkspace {
    ctx: Context<u8, i32, SingleThreaded>,
}

impl SaisWorkspace {
    pub(crate) fn new() -> Self {
        Self {
            ctx: Context::new_single_threaded(),
        }
    }

    #[cfg(not(feature = "paranoid"))]
    pub(crate) fn suffix_array_with_rank(
        &mut self,
        input: &[u8],
        sa: &mut [i32],
        rank: &mut [u32],
    ) {
        debug_assert_eq!(input.len(), sa.len());
        debug_assert_eq!(input.len(), rank.len());
        if input.is_empty() {
            return;
        }

        SuffixArrayConstruction::for_text(input)
            .in_borrowed_buffer(sa)
            .single_threaded()
            .with_context(&mut self.ctx)
            .run()
            .expect("libsais suffix-array construction failed");

        let sa_ptr = sa.as_ptr();
        let rank_ptr = rank.as_mut_ptr();
        let len = sa.len();
        for i in 0..len {
            // SAFETY: `i` is in bounds and libsais wrote a valid suffix
            // permutation into `sa`.
            unsafe {
                if i + PREFETCH_DISTANCE < len {
                    let next = *sa_ptr.add(i + PREFETCH_DISTANCE);
                    debug_assert!(next >= 0);
                    #[cfg(target_arch = "x86_64")]
                    _mm_prefetch(rank_ptr.add(next as u32 as usize).cast::<i8>(), _MM_HINT_T0);
                    #[cfg(not(target_arch = "x86_64"))]
                    prefetch(rank, next as u32 as usize);
                }
                let pos = *sa_ptr.add(i);
                debug_assert!(pos >= 0);
                *rank_ptr.add(pos as u32 as usize) = i as u32;
            }
        }
    }

    #[cfg(feature = "paranoid")]
    pub(crate) fn suffix_array_with_rank(
        &mut self,
        input: &[u8],
        sa: &mut [i32],
        rank: &mut [u32],
    ) {
        debug_assert_eq!(input.len(), sa.len());
        debug_assert_eq!(input.len(), rank.len());
        if input.is_empty() {
            return;
        }

        SuffixArrayConstruction::for_text(input)
            .in_borrowed_buffer(sa)
            .single_threaded()
            .with_context(&mut self.ctx)
            .run()
            .expect("libsais suffix-array construction failed");

        for (i, &pos) in sa.iter().enumerate() {
            debug_assert!(pos >= 0);
            rank[pos as usize] = i as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suffix_array(input: &[u8]) -> Vec<i32> {
        let mut workspace = SaisWorkspace::new();
        let mut sa = vec![0; input.len()];
        let mut rank = vec![0u32; input.len()];
        workspace.suffix_array_with_rank(input, &mut sa, &mut rank);
        for (i, &pos) in sa.iter().enumerate() {
            assert_eq!(rank[pos as usize], i as u32);
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
