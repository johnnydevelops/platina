use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

thread_local! {
    static COUNTER: RefCell<u32> = const { RefCell::new(0) };
}

static CONFIG: OnceLock<String> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind { A, B(u8), C { x: i32 } }

pub fn run() {
    // hashing collections (order-independent output)
    let mut m: HashMap<&str, Vec<u32>> = HashMap::new();
    for (i, w) in "the cat sat on the mat the end".split(' ').enumerate() {
        m.entry(w).or_default().push(i as u32);
    }
    let mut keys: Vec<_> = m.iter().map(|(k, v)| format!("{k}:{v:?}")).collect();
    keys.sort();
    println!("{}", keys.join(" "));
    let hs: HashSet<Kind> = [Kind::A, Kind::B(2), Kind::C { x: -1 }, Kind::A].into_iter().collect();
    let mut hv: Vec<_> = hs.into_iter().collect();
    hv.sort();
    println!("{hv:?}");

    // ordered + other collections
    let bt: BTreeMap<i32, char> = (0..6).map(|i| (i * i - 10, (b'a' + i as u8) as char)).collect();
    println!("{bt:?} {:?}", bt.range(-5..20).map(|(k, _)| k).collect::<Vec<_>>());
    let bs: BTreeSet<u8> = b"mississippi".iter().copied().collect();
    println!("{:?}", bs.iter().map(|b| *b as char).collect::<String>());
    let mut dq: VecDeque<i32> = VecDeque::new();
    for i in 0..5 { if i % 2 == 0 { dq.push_back(i) } else { dq.push_front(i) } }
    println!("{dq:?}");
    let mut heap = BinaryHeap::from(vec![5, 1, 8, 3, 9, 2]);
    let mut top = vec![];
    while let Some(x) = heap.pop() { top.push(x); if top.len() == 3 { break } }
    println!("{top:?}");

    // strings
    let mut s = String::new();
    for i in 0..3 { write!(s, "[{i:>3}|{:<4}|{:^5}]", i * 7, "ab").unwrap(); }
    println!("{s}");
    println!("{:?}", "a,b,,c".split(',').map(str::to_uppercase).collect::<Vec<_>>());
    println!("{} {} {}", 255u8.to_string(), format!("{:08.3}", -3.14159), format!("{:#010b}", 37));
    println!("{}", "héllo wörld".char_indices().filter(|(_, c)| !c.is_ascii()).map(|(i, _)| i.to_string()).collect::<Vec<_>>().join(","));

    // shared state
    let shared = Arc::new(Mutex::new(Vec::new()));
    for i in 0..3 { shared.lock().unwrap().push(i * 10); }
    let rw = RwLock::new(5);
    *rw.write().unwrap() += 1;
    println!("{:?} {}", shared.lock().unwrap(), *rw.read().unwrap());
    COUNTER.with(|c| *c.borrow_mut() += 3);
    COUNTER.with(|c| *c.borrow_mut() *= 2);
    println!("tls {}", COUNTER.with(|c| *c.borrow()));
    let cfg = CONFIG.get_or_init(|| "cfg".repeat(2));
    println!("{cfg} {}", CONFIG.get().is_some());
    let rc = Rc::new(RefCell::new(1));
    let rc2 = Rc::clone(&rc);
    *rc2.borrow_mut() += 41;
    println!("{} {}", rc.borrow(), Rc::strong_count(&rc));

    // time
    let t0 = Instant::now();
    let mut acc = 0u64;
    for i in 0..10000u64 { acc = acc.wrapping_add(i * i); }
    let el = t0.elapsed();
    println!("{acc} {} {}", el < Duration::from_secs(5), Duration::from_millis(1500).as_secs_f64());

    // errors / parsing
    let r: Result<Vec<i32>, _> = "1 2 x 4".split(' ').map(str::parse::<i32>).collect();
    println!("{}", r.unwrap_err());
    let ok: Result<Vec<f64>, _> = "1.5 -2 3e2".split(' ').map(str::parse::<f64>).collect();
    println!("{:?}", ok);
    let mut v = vec![3.2f64, -1.0, 7.5, 0.0];
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{v:?} {}", v.iter().sum::<f64>());
}
