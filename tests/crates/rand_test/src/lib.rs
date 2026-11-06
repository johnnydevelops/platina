use rand_chacha::ChaCha8Rng as SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

pub fn run() {
    let mut rng = SmallRng::seed_from_u64(42);
    let xs: Vec<u32> = (0..5).map(|_| rng.random()).collect();
    println!("{xs:?}");
    let ys: Vec<i32> = (0..8).map(|_| rng.random_range(-100..100)).collect();
    println!("{ys:?}");
    let f: f64 = rng.random();
    println!("{f:.12}");
    let mut deck: Vec<u8> = (1..=10).collect();
    deck.shuffle(&mut rng);
    println!("{deck:?} {}", rng.random_bool(0.5));
}
