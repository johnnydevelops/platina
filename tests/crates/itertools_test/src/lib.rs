use itertools::Itertools;

pub fn run() {
    println!("{:?}", (1..=4).combinations(2).collect::<Vec<_>>());
    println!("{:?}", "abc".chars().permutations(3).map(|p| p.into_iter().collect::<String>()).collect::<Vec<_>>());
    println!("{}", [3, 1, 4, 1, 5, 9, 2, 6].iter().sorted().dedup().join("-"));
    let groups: Vec<(bool, Vec<i32>)> = [1, 3, 5, 2, 4, 7].iter().chunk_by(|x| *x % 2 == 0).into_iter().map(|(k, g)| (k, g.copied().collect())).collect();
    println!("{groups:?}");
    println!("{:?}", (0..10).tuple_windows::<(_, _, _)>().map(|(a, b, c)| a * b + c).collect::<Vec<_>>());
    println!("{:?}", ["x", "y"].iter().cartesian_product(1..=2).collect::<Vec<_>>());
    println!("{:?}", [5, 3, 8, 1].iter().minmax());
}
