//! The `random` provider's generator (testing.md, Randomness): xoshiro256**
//! seeded through SplitMix64. `runtime.c` implements the same generator, so a
//! seed draws the same numbers on both backends.

/// One xoshiro256** state.
#[derive(Clone, Debug)]
pub(crate) struct Generator {
    state: [u64; 4],
}

impl Generator {
    /// The generator a 64-bit seed names: four SplitMix64 outputs from `seed`.
    pub(crate) fn seeded(seed: u64) -> Self {
        let mut mix = seed;
        Self {
            state: std::array::from_fn(|_| splitmix64(&mut mix)),
        }
    }

    /// A generator seeded from the operating system, for a program's `main`.
    /// Not suitable for keys or tokens.
    pub(crate) fn from_os() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos()),
        );
        Self::seeded(hasher.finish())
    }

    /// The next 64 bits, as `Random.next_int` returns them.
    pub(crate) fn next(&mut self) -> u64 {
        let [a, b, c, d] = &mut self.state;
        let result = b.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let shifted = *b << 17;
        *c ^= *a;
        *d ^= *b;
        *b ^= *c;
        *a ^= *d;
        *c ^= shifted;
        *d = d.rotate_left(45);
        result
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}

/// A test's seed: the run seed mixed with the FNV-1a hash of the test's
/// identity, so a test draws the same numbers alone, filtered, or among
/// others, and adding a test changes no other test's numbers.
pub(crate) fn test_seed(run_seed: u64, module: &str, name: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in module.bytes().chain([0]).chain(name.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut mix = run_seed ^ hash;
    splitmix64(&mut mix)
}

#[cfg(test)]
mod tests {
    use super::{Generator, test_seed};

    /// The reference xoshiro256** output for the state SplitMix64 makes from 0.
    #[test]
    fn the_generator_matches_the_reference_sequence() {
        let mut generator = Generator::seeded(0);
        let first: Vec<u64> = (0..3).map(|_| generator.next()).collect();
        assert_eq!(
            first,
            [
                0x99ec_5f36_cb75_f2b4,
                0xbf6e_1f78_4956_452a,
                0x1a5f_849d_4933_e6e0
            ]
        );
    }

    #[test]
    fn a_test_seed_depends_on_the_run_seed_and_the_identity() {
        let seed = test_seed(0, "main", "draws");
        assert_eq!(seed, test_seed(0, "main", "draws"));
        assert_ne!(seed, test_seed(1, "main", "draws"));
        assert_ne!(seed, test_seed(0, "main", "draws again"));
        assert_ne!(test_seed(0, "ab", "c"), test_seed(0, "a", "bc"));
    }
}
