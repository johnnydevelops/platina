// features: allocator_api
// Regression test: `std::alloc::System`. In release builds rustc's MIR inliner folds
// `<System as GlobalAlloc>`, and dlmalloc's own `#[inline]` methods, into std's code, e.g.
// `System::alloc_impl` (`Arc<thread::Inner, System>` from `thread::current()`) and `grow_impl`.
// The backend must route every such path to the runtime's System heap instead of lowering
// dlmalloc (which needs the linker's `__heap_base`, or runs on runtime-heap memory).

use std::alloc::{Allocator, GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::thread;

pub fn run() {
    // thread::current(): std allocates its thread handle as Arc<_, System>
    let t = thread::current();
    println!("thread id stable: {}", t.id() == thread::current().id());

    // Arc in System: allocate, clone, drop
    let a = Arc::new_in([1u64, 2, 3], System);
    let b = a.clone();
    println!("arc strong {} sum {}", Arc::strong_count(&a), b.iter().sum::<u64>());
    drop(a);
    println!("arc strong after drop {}", Arc::strong_count(&b));

    // Vec in System: grow (realloc) past the initial capacity, then shrink
    let mut v: Vec<u32, System> = Vec::with_capacity_in(4, System);
    for i in 0..1000 {
        v.push(i * 3);
    }
    let sum: u64 = v.iter().map(|&x| x as u64).sum();
    v.truncate(10);
    v.shrink_to_fit();
    println!("vec sum {sum} len {} cap>=10 {} head {:?}", v.len(), v.capacity() >= 10, &v[..4]);

    unsafe {
        // grow_zeroed (same alignment: realloc + zero fill)
        let small = Layout::from_size_align(16, 8).unwrap();
        let p = System.allocate_zeroed(small).unwrap();
        let big = Layout::from_size_align(64, 8).unwrap();
        let q = System.grow_zeroed(p.cast(), small, big).unwrap();
        let bytes = std::slice::from_raw_parts_mut(q.cast::<u8>().as_ptr(), 64);
        println!("grow_zeroed zeroed {} len {}", bytes.iter().all(|&x| x == 0), q.len());
        for (i, x) in bytes.iter_mut().enumerate() {
            *x = i as u8;
        }
        // grow to a larger alignment (allocate + copy + deallocate)
        let wide = Layout::from_size_align(128, 16).unwrap();
        let r = System.grow(q.cast(), big, wide).unwrap();
        let rb = std::slice::from_raw_parts(r.cast::<u8>().as_ptr(), 64);
        println!("grow realigned {} kept {}", r.cast::<u8>().as_ptr() as usize % 16 == 0, rb.iter().enumerate().all(|(i, &x)| x == i as u8));
        // shrink (same alignment: realloc)
        let narrow = Layout::from_size_align(32, 16).unwrap();
        let s = System.shrink(r.cast(), wide, narrow).unwrap();
        let sb = std::slice::from_raw_parts(s.cast::<u8>().as_ptr(), 32);
        println!("shrink kept {}", sb.iter().enumerate().all(|(i, &x)| x == i as u8));
        System.deallocate(s.cast(), narrow);

        // zero-size: a dangling, aligned pointer; deallocating it is a no-op
        let zl = Layout::from_size_align(0, 8).unwrap();
        let z = System.allocate(zl).unwrap();
        println!("zero-size dangling {} len {}", z.cast::<u8>().as_ptr() as usize == 8, z.len());
        System.deallocate(z.cast(), zl);

        // GlobalAlloc through a function pointer (the methods' bodies, not a direct call)
        let alloc: unsafe fn(&System, Layout) -> *mut u8 = <System as GlobalAlloc>::alloc;
        let dealloc: unsafe fn(&System, *mut u8, Layout) = <System as GlobalAlloc>::dealloc;
        let l = Layout::from_size_align(24, 8).unwrap();
        let m = std::hint::black_box(alloc)(&System, l);
        m.write_bytes(7, 24);
        println!("fn-pointer alloc ok {}", !m.is_null() && *m.add(23) == 7);
        std::hint::black_box(dealloc)(&System, m, l);
    }
}
