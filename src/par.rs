//! Work shared among scoped threads, for jobs whose result does not depend
//! on how many threads run them.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The number of threads to use for `threads` (0: the machine's available
/// parallelism).
pub(crate) fn threads(threads: usize) -> usize {
    if threads != 0 {
        return threads;
    }
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// `f(0), f(1), ..., f(tasks - 1)`, computed on up to `threads` threads (0:
/// automatic), in order. Falls back to the calling thread alone where
/// threads cannot be spawned.
pub(crate) fn map<T: Send>(tasks: usize, threads: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let workers = self::threads(threads).min(tasks);
    if workers <= 1 {
        return (0..tasks).map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let run = || {
        let mut done = Vec::new();
        loop {
            let t = next.fetch_add(1, Ordering::Relaxed);
            if t >= tasks {
                break;
            }
            done.push((t, f(t)));
        }
        done
    };
    let mut results: Vec<Option<T>> = (0..tasks).map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> =
            (1..workers).filter_map(|_| std::thread::Builder::new().spawn_scoped(s, run).ok()).collect();
        for (t, r) in run() {
            results[t] = Some(r);
        }
        for h in handles {
            match h.join() {
                Ok(done) => {
                    for (t, r) in done {
                        results[t] = Some(r);
                    }
                }
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
    });
    results.into_iter().map(|r| r.expect("every task ran")).collect()
}

/// Splits `data`, rows of `row_len`, into contiguous bands of rows and runs
/// `f(rows, band)` on each, on up to `threads` threads (0: automatic).
/// Small jobs run on the calling thread, as does everything where threads
/// cannot be spawned.
pub(crate) fn bands<T: Send>(
    data: &mut [T],
    row_len: usize,
    threads: usize,
    f: impl Fn(std::ops::Range<usize>, &mut [T]) + Sync,
) {
    /// Below this many elements a band is not worth a thread.
    const MIN_BAND: usize = 1 << 16;
    let rows = data.len().checked_div(row_len).unwrap_or(0);
    let workers = self::threads(threads).min(data.len() / MIN_BAND).min(rows);
    if workers <= 1 {
        f(0..rows, data);
        return;
    }
    // A few bands per thread, so that uneven bands even out.
    let per = rows.div_ceil(workers * 4);
    type Task<'a, T> = std::sync::Mutex<Option<(std::ops::Range<usize>, &'a mut [T])>>;
    let tasks: Vec<Task<T>> = data
        .chunks_mut(per * row_len)
        .enumerate()
        .map(|(i, band)| std::sync::Mutex::new(Some((i * per..i * per + band.len() / row_len, band))))
        .collect();
    let next = AtomicUsize::new(0);
    let run = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(task) = tasks.get(i) else { break };
            let taken = task.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some((ys, band)) = taken {
                f(ys, band);
            }
        }
    };
    std::thread::scope(|s| {
        for _ in 1..workers {
            // Where a thread cannot be had, the others (and this one) do
            // its share.
            let _ = std::thread::Builder::new().spawn_scoped(s, run);
        }
        run();
    });
}
