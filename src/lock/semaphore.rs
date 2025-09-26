use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct Semaphore {
    mutex: Mutex<isize>,
    cond: Condvar,
    max: isize,
}

impl Semaphore {
    fn new(max: isize) -> Self {
        Self {
            mutex: Mutex::new(0),
            cond: Condvar::new(),
            max,
        }
    }

    fn wait(&self) {
        let mut count = self.mutex.lock().unwrap();
        while *count >= self.max {
            count = self.cond.wait(count).unwrap();
        }
        *count += 1;
    }

    fn post(&self) {
        let mut count = self.mutex.lock().unwrap();
        *count -= 1;
        self.cond.notify_one(); // *count < self.max
    }
}

static N_THREADS_IN_LOCK: AtomicUsize = AtomicUsize::new(0);

pub fn run() {
    const NUM_LOOP: usize = 10;
    const NUM_THREADS: usize = 4;
    const SEMAPHORE_SIZE: isize = 4;

    let semaphore = Arc::new(Semaphore::new(SEMAPHORE_SIZE));
    let mut v = Vec::with_capacity(NUM_THREADS);
    for i in 0..NUM_THREADS {
        let s = semaphore.clone();
        let thread = thread::spawn(move || {
            for _ in 0..NUM_LOOP {
                s.wait();
                N_THREADS_IN_LOCK.fetch_add(1, Ordering::SeqCst);
                let n = N_THREADS_IN_LOCK.load(Ordering::SeqCst);
                println!("Semaphore: i = {}, #threads in lock = {}", i, n);
                N_THREADS_IN_LOCK.fetch_sub(1, Ordering::SeqCst);
                s.post();
            }
        });
        v.push(thread);
    }

    for thread in v {
        thread.join().unwrap();
    }
}
