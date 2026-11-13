//! Test-only helpers: a tiny deterministic generator used by the generated
//! round-trip tests to fill every packet with non-trivial data.

use crate::codec::{NbtTag, NetworkNbt};

/// Deterministic pseudo-random generator (xorshift64*), with a budget that
/// bounds the amount of optional / repeated data so samples stay small even
/// for recursive types.
pub struct Gen {
    state: u64,
    budget: u32,
}

impl Gen {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1, budget: 600 }
    }

    pub fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform-ish value in `0..bound` (`bound` must not be zero).
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn spend(&mut self) -> bool {
        if self.budget == 0 {
            false
        } else {
            self.budget -= 1;
            true
        }
    }

    pub fn option<T>(&mut self, make: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.spend() && self.below(2) == 0 {
            Some(make(self))
        } else {
            None
        }
    }

    pub fn vec<T>(&mut self, make: impl FnMut(&mut Self) -> T) -> Vec<T> {
        let len = if self.spend() { self.below(3) } else { 0 };
        self.vec_of(len, make)
    }

    pub fn vec_of<T>(&mut self, len: usize, mut make: impl FnMut(&mut Self) -> T) -> Vec<T> {
        (0..len).map(|_| make(self)).collect()
    }
}

/// Builds a pseudo-random value.
pub trait Sample {
    fn sample(g: &mut Gen) -> Self;
}

macro_rules! sample_ints {
    ($($ty:ty),*) => {$(
        impl Sample for $ty {
            fn sample(g: &mut Gen) -> Self {
                // Mix small values (short varints) with full-width ones.
                match g.below(4) {
                    0 => (g.next() % 128) as $ty,
                    1 => <$ty>::MAX,
                    2 => <$ty>::MIN,
                    _ => g.next() as $ty,
                }
            }
        }
    )*};
}

sample_ints!(u8, i8, u16, i16, u32, i32, u64, i64);

impl Sample for bool {
    fn sample(g: &mut Gen) -> Self {
        g.below(2) == 0
    }
}

impl Sample for f32 {
    fn sample(g: &mut Gen) -> Self {
        // Finite values only: NaN is never equal to itself.
        (g.next() % 20_001) as f32 / 8.0 - 1250.0
    }
}

impl Sample for f64 {
    fn sample(g: &mut Gen) -> Self {
        (g.next() % 2_000_001) as f64 / 64.0 - 15_625.0
    }
}

impl Sample for String {
    fn sample(g: &mut Gen) -> Self {
        const WORDS: &[&str] = &["", "a", "minecraft:stone", "héllo wörld", "§l bold", "{\"json\":true}"];
        WORDS[g.below(WORDS.len())].to_owned()
    }
}

impl Sample for NetworkNbt {
    fn sample(g: &mut Gen) -> Self {
        let root = match g.below(3) {
            0 => NbtTag::Compound(Vec::new()),
            1 => NbtTag::Compound(vec![
                ("id".to_owned(), NbtTag::Int(Sample::sample(g))),
                ("name".to_owned(), NbtTag::String(Sample::sample(g))),
                ("list".to_owned(), NbtTag::List(vec![NbtTag::Long(Sample::sample(g)), NbtTag::Long(1)])),
            ]),
            _ => NbtTag::Compound(vec![("flag".to_owned(), NbtTag::Byte(Sample::sample(g)))]),
        };
        NetworkNbt::new(root)
    }
}
