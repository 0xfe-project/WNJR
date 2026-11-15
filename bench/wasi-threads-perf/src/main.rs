//! Three 4-thread benchmarks, built once for the host and once for
//! wasm32-wasip1-threads.
//!
//! Same source, same work, same arithmetic order in every runtime, so the
//! checksums must match bit for bit across the host, WNJR, Node and Bun. A
//! runtime that disagrees is wrong, not fast.
//!
//! Usage: wt-perf <mergesort|matmul|quicksort|pi|all> [size] [threads]
//!
//! Every run prints one machine-readable line per benchmark:
//!   RESULT bench=<name> threads=<n> n=<n> gen_ms=<f> work_ms=<f> checksum=<hex>

use std::time::Instant;

const THREADS: usize = 4;

// ---------------------------------------------------------------------------
// deterministic pseudo-randomness: no crates, identical everywhere
// ---------------------------------------------------------------------------

#[inline]
fn xorshift64(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

#[inline]
fn unit_f64(x: u64) -> f64 {
    // top 53 bits into [0, 1)
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

// ---------------------------------------------------------------------------
// malloc/free exports, for hosts that speak the single-argument
// wasi.thread-spawn ABI
// ---------------------------------------------------------------------------
//
// Rust's std uses the old ABI: thread-spawn(start_arg) returns the tid, and the
// host has nowhere to report an error but the return value. @emnapi/wasi-threads
// supports it by allocating an 8-byte error/tid cell in the guest, which needs
// malloc and free exported from the module (Emscripten modules export them
// already; Rust ones do not). Both are fixed 8-byte cells, so the layout is
// known at the call site.

// The exports are turned on by .cargo/config.toml: Rust already links dlmalloc
// into a wasip1-threads module, it just does not export malloc and free.

// ---------------------------------------------------------------------------
// 1. parallel merge sort
// ---------------------------------------------------------------------------

/// Top-down merge sort into a fresh buffer.
fn merge_sort(v: &[u32]) -> Vec<u32> {
    let mut out = v.to_vec();
    let mut scratch = vec![0u32; v.len()];
    let len = out.len();
    if len > 1 {
        sort_range(&mut out, &mut scratch, 0, len);
    }
    out
}

fn sort_range(buf: &mut [u32], scratch: &mut [u32], lo: usize, hi: usize) {
    if hi - lo <= 1 {
        return;
    }
    if hi - lo <= 16 {
        insertion_sort(&mut buf[lo..hi]);
        return;
    }
    let mid = lo + (hi - lo) / 2;
    sort_range(buf, scratch, lo, mid);
    sort_range(buf, scratch, mid, hi);
    merge_into(buf, scratch, lo, mid, hi);
}

fn insertion_sort(s: &mut [u32]) {
    for i in 1..s.len() {
        let v = s[i];
        let mut j = i;
        while j > 0 && s[j - 1] > v {
            s[j] = s[j - 1];
            j -= 1;
        }
        s[j] = v;
    }
}

/// Merge buf[lo..mid] and buf[mid..hi] (both sorted) into scratch[lo..hi],
/// then copy back.
fn merge_into(buf: &mut [u32], scratch: &mut [u32], lo: usize, mid: usize, hi: usize) {
    let (mut i, mut j, mut k) = (lo, mid, lo);
    while i < mid && j < hi {
        if buf[i] <= buf[j] {
            scratch[k] = buf[i];
            i += 1;
        } else {
            scratch[k] = buf[j];
            j += 1;
        }
        k += 1;
    }
    while i < mid {
        scratch[k] = buf[i];
        i += 1;
        k += 1;
    }
    while j < hi {
        scratch[k] = buf[j];
        j += 1;
        k += 1;
    }
    buf[lo..hi].copy_from_slice(&scratch[lo..hi]);
}

/// Merge two already-sorted vectors.
fn merge_two(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] <= b[j] {
            out.push(a[i]);
            i += 1;
        } else {
            out.push(b[j]);
            j += 1;
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

fn bench_mergesort(n: usize, threads: usize) -> (f64, f64, u64, usize) {
    let t_gen = Instant::now();
    let mut data = Vec::with_capacity(n);
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..n {
        s = xorshift64(s);
        data.push((s >> 32) as u32);
    }
    let gen_ms = ms(t_gen.elapsed());

    let chunk = n.div_ceil(threads);

    let work = Instant::now();
    let sorted = std::thread::scope(|sc| {
        let mut slots: Vec<Option<Vec<u32>>> = (0..threads).map(|_| None).collect();
        let mut handles = Vec::new();
        for (i, slot) in slots.iter_mut().enumerate() {
            let lo = i * chunk;
            let hi = (lo + chunk).min(n);
            if lo >= hi {
                continue;
            }
            let part = &data[lo..hi];
            if i == 0 {
                *slot = Some(merge_sort(part));
            } else {
                handles.push(sc.spawn(move || merge_sort(part)));
            }
        }
        for (h, i) in handles.into_iter().zip(1..) {
            slots[i] = Some(h.join().unwrap());
        }
        // tree merge: round 1 in parallel over the spawned lanes, last round
        // on this thread
        let mut level: Vec<Vec<u32>> = slots.into_iter().flatten().collect();
        while level.len() > 1 {
            let mut next: Vec<Vec<u32>> = Vec::with_capacity(level.len().div_ceil(2));
            let mut pending = Vec::new();
            let mut iter = level.into_iter();
            while let Some(a) = iter.next() {
                match iter.next() {
                    Some(b) => {
                        pending.push(sc.spawn(move || merge_two(&a, &b)));
                        next.push(Vec::new());
                    }
                    None => next.push(a),
                }
            }
            let mut k = 0;
            for h in pending {
                let merged = h.join().unwrap();
                while !next[k].is_empty() {
                    k += 1;
                }
                next[k] = merged;
                k += 1;
            }
            level = next;
        }
        level.pop().unwrap()
    });
    let work_ms = ms(work.elapsed());

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut sorted_ok = true;
    for (i, v) in sorted.iter().enumerate() {
        if i > 0 && sorted[i - 1] > *v {
            sorted_ok = false;
        }
        h = (h ^ *v as u64).wrapping_mul(0x100_0000_01b3);
    }
    assert!(sorted_ok, "mergesort output is not sorted");
    assert_eq!(sorted.len(), n, "mergesort lost elements");
    (gen_ms, work_ms, h, n)
}

// ---------------------------------------------------------------------------
// 2. matrix multiplication, C = A * B, f64, row blocks
// ---------------------------------------------------------------------------

/// Multiply rows [row_base, row_base + c.len()/n) of A by B into the c block,
/// which holds exactly those rows.
fn matmul_block(a: &[f64], b: &[f64], c: &mut [f64], n: usize, row_base: usize) {
    let rows = c.len() / n;
    for i in 0..rows {
        let crow = &mut c[i * n..(i + 1) * n];
        for k in 0..n {
            let aik = a[(row_base + i) * n + k];
            if aik == 0.0 {
                continue;
            }
            let brow = &b[k * n..(k + 1) * n];
            for j in 0..n {
                crow[j] += aik * brow[j];
            }
        }
    }
}

fn bench_matmul(n: usize, threads: usize) -> (f64, f64, u64, usize) {
    let t_gen = Instant::now();
    let mut a = vec![0f64; n * n];
    let mut b = vec![0f64; n * n];
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    for v in a.iter_mut() {
        s = xorshift64(s);
        *v = unit_f64(s) - 0.5;
    }
    for v in b.iter_mut() {
        s = xorshift64(s);
        *v = unit_f64(s) - 0.5;
    }
    let mut c = vec![0f64; n * n];
    let gen_ms = ms(t_gen.elapsed());

    let rows = n.div_ceil(threads);
    let work = Instant::now();
    std::thread::scope(|sc| {
        let mut it = c.chunks_mut(rows * n);
        let first = it.next().unwrap();
        let rest: Vec<&mut [f64]> = it.collect();
        let mut handles = Vec::new();
        for (idx, block) in rest.into_iter().enumerate() {
            let (a, b) = (&a, &b);
            let row_base = (idx + 1) * rows;
            handles.push(sc.spawn(move || matmul_block(a, b, block, n, row_base)));
        }
        matmul_block(&a, &b, first, n, 0);
        for h in handles {
            h.join().unwrap();
        }
    });
    let work_ms = ms(work.elapsed());

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in c.iter() {
        h = (h ^ v.to_bits()).wrapping_mul(0x100_0000_01b3);
    }
    let ops = 2.0 * (n as f64) * (n as f64) * (n as f64);
    let _ = ops;
    (gen_ms, work_ms, h, n)
}

// ---------------------------------------------------------------------------
// 3. quicksort, single threaded - the comparison with the three above is the
//    one that has nothing to do with threads: same JIT, same code, one core.
// ---------------------------------------------------------------------------

/// Hoare partition, median-of-three pivot. Returns the split point j, so the
/// halves are [lo, j + 1) and [j + 1, hi).
fn partition(s: &mut [u32], lo: usize, hi: usize) -> usize {
    let mid = lo + (hi - lo) / 2;
    let pivot = if s[mid] < s[lo] {
        if s[hi - 1] < s[mid] {
            s[mid]
        } else if s[hi - 1] < s[lo] {
            s[hi - 1]
        } else {
            s[lo]
        }
    } else if s[hi - 1] < s[lo] {
        s[lo]
    } else if s[hi - 1] < s[mid] {
        s[hi - 1]
    } else {
        s[mid]
    };
    let mut i = lo;
    let mut j = hi - 1;
    loop {
        while s[i] < pivot {
            i += 1;
        }
        while s[j] > pivot {
            j -= 1;
        }
        if i >= j {
            return j;
        }
        s.swap(i, j);
        i += 1;
        j -= 1;
    }
}

fn quicksort(s: &mut [u32]) {
    // An explicit stack: no recursion depth to worry about on adversarial input.
    let mut stack: Vec<(usize, usize)> = vec![(0, s.len())];
    while let Some((lo, hi)) = stack.pop() {
        if hi - lo <= 1 {
            continue;
        }
        if hi - lo <= 24 {
            insertion_sort(&mut s[lo..hi]);
            continue;
        }
        let j = partition(s, lo, hi);
        stack.push((lo, j + 1));
        stack.push((j + 1, hi));
    }
}

fn bench_quicksort(n: usize, _threads: usize) -> (f64, f64, u64, usize) {
    let t_gen = Instant::now();
    let mut data = Vec::with_capacity(n);
    let mut s = 0xC2B2_AE3D_27D4_EB4Fu64;
    for _ in 0..n {
        s = xorshift64(s);
        data.push((s >> 32) as u32);
    }
    let gen_ms = ms(t_gen.elapsed());

    let work = Instant::now();
    quicksort(&mut data);
    let work_ms = ms(work.elapsed());

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut sorted_ok = true;
    for (i, v) in data.iter().enumerate() {
        if i > 0 && data[i - 1] > *v {
            sorted_ok = false;
        }
        h = (h ^ *v as u64).wrapping_mul(0x100_0000_01b3);
    }
    assert!(sorted_ok, "quicksort output is not sorted");
    assert_eq!(data.len(), n, "quicksort lost elements");
    (gen_ms, work_ms, h, n)
}

// ---------------------------------------------------------------------------
// 4. Monte Carlo pi
// ---------------------------------------------------------------------------

/// Sample i of the global sequence, whatever thread ended up with it. Hashing the
/// index instead of continuing a per-thread stream is what makes the hit count -
/// and so the checksum - independent of how the range is split.
#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn pi_block(base: u64, start: usize, samples: usize) -> u64 {
    let mut hits = 0u64;
    for i in start..start + samples {
        let x = unit_f64(splitmix64(base ^ i as u64));
        let y = unit_f64(splitmix64(base ^ (i as u64).wrapping_add(1 << 32)));
        if x * x + y * y <= 1.0 {
            hits += 1;
        }
    }
    hits
}

fn bench_pi(n: usize, threads: usize) -> (f64, f64, u64, usize) {
    const BASE: u64 = 0x1234_5678_9ABC_DEF1;
    let per = n.div_ceil(threads);
    let work = Instant::now();
    let hits = std::thread::scope(|sc| {
        let mut handles = Vec::new();
        for t in 1..threads {
            let start = t * per;
            handles.push(sc.spawn(move || pi_block(BASE, start, per)));
        }
        let mut total = pi_block(BASE, 0, per);
        for h in handles {
            total += h.join().unwrap();
        }
        total
    });
    let work_ms = ms(work.elapsed());

    let pi = 4.0 * hits as f64 / (per * threads) as f64;
    // checksum covers the count, not the float, so it is exact everywhere
    let h: u64 = hits;
    println!("  pi ~= {:.6} (hits {} / {})", pi, hits, per * threads);
    (0.0, work_ms, h, per * threads)
}

// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let which = args.get(1).cloned().unwrap_or_else(|| "all".to_string());
    let size: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let threads: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(THREADS);

    // Two passes: the first one pays whatever the runtime charges for turning
    // code into machine code, the second one is pure execution. Reporting both
    // is the only way to compare a JIT that compiles lazily against one that
    // compiles the whole module at instantiation.
    let run = |name: &str, size: usize, threads: usize, f: fn(usize, usize) -> (f64, f64, u64, usize)| {
        let (gen_ms, cold_ms, cold_sum, n) = f(size, threads);
        let (_, work_ms, warm_sum, _) = f(size, threads);
        assert_eq!(cold_sum, warm_sum, "{} is not deterministic", name);
        println!(
            "RESULT bench={} threads={} n={} gen_ms={:.3} cold_ms={:.3} work_ms={:.3} checksum={:016x}",
            name, threads, n, gen_ms, cold_ms, work_ms, warm_sum
        );
    };

    if which == "mergesort" || which == "all" {
        run("mergesort", if size > 0 { size } else { 4_000_000 }, threads, bench_mergesort);
    }

    if which == "matmul" || which == "all" {
        run("matmul", if size > 0 { size } else { 768 }, threads, bench_matmul);
    }
    if which == "quicksort" || which == "all" {
        // Single threaded by definition, whatever the argument says.
        run("quicksort", if size > 0 { size } else { 4_000_000 }, 1, bench_quicksort);
    }
    if which == "pi" || which == "all" {
        run("pi", if size > 0 { size } else { 40_000_000 }, threads, bench_pi);
    }
}
