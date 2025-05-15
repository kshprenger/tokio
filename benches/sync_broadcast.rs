use rand::{Rng, RngCore, SeedableRng};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Barrier, Notify};

use criterion::measurement::WallTime;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkGroup, Criterion};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(6)
        .build()
        .unwrap()
}

fn do_work(rng: &mut impl RngCore) -> u32 {
    use std::fmt::Write;
    let mut message = String::new();
    for i in 1..=10 {
        let _ = write!(&mut message, " {i}={}", rng.gen::<f64>());
    }
    message
        .as_bytes()
        .iter()
        .map(|&c| c as u32)
        .fold(0, u32::wrapping_add)
}

fn contention_impl<const N_SENDERS: usize, const N_RECIEVERS: usize>(
    g: &mut BenchmarkGroup<WallTime>,
) {
    let rt = rt();

    g.bench_function(
        format!("{}/{}", N_SENDERS.to_string(), N_RECIEVERS.to_string()),
        |b| {
            b.iter(|| {
                let (tx, _rx) = broadcast::channel::<usize>(1000);
                let wg = Arc::new(AtomicUsize::new(0));

                const N_ITERS: usize = 10;
                let barrier = Arc::new(tokio::sync::Barrier::new(N_SENDERS + N_RECIEVERS + 1));

                for n in 0..N_RECIEVERS {
                    let wg = wg.clone();
                    let mut rx = tx.subscribe();
                    let barrier = barrier.clone();
                    let mut rng = rand::rngs::StdRng::seed_from_u64(n as u64);
                    rt.spawn(async move {
                        barrier.wait().await;
                        while (rx.recv().await).is_ok() {
                            let r = do_work(&mut rng);
                            let _ = black_box(r);
                            wg.fetch_add(1, Ordering::Relaxed);
                        }
                    });
                }

                for _ in 0..N_SENDERS {
                    let tx = tx.clone();
                    let barrier = barrier.clone();
                    rt.spawn(async move {
                        barrier.wait().await;
                        for i in 0..N_ITERS {
                            tx.send(i).unwrap();
                        }
                    });
                }

                rt.block_on(async move {
                    barrier.wait().await;
                    while wg.load(Ordering::Relaxed) != N_SENDERS * N_ITERS * N_RECIEVERS {
                        std::hint::spin_loop();
                    }
                })
            })
        },
    );
}

fn bench_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    contention_impl::<1, 1>(&mut group);
    contention_impl::<4, 1>(&mut group);
    contention_impl::<32, 1>(&mut group);
    contention_impl::<1, 4>(&mut group);
    contention_impl::<1, 32>(&mut group);
    contention_impl::<4, 4>(&mut group);
    contention_impl::<32, 32>(&mut group);
    group.finish();
}

criterion_group!(contention, bench_contention);

criterion_main!(contention);
