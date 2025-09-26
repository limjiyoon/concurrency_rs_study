use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct BakeryLock {
    size: usize,
    entering: Vec<AtomicBool>,
    tickets: Vec<AtomicUsize>,
}

impl BakeryLock {
    fn new(size: usize) -> Self {
        Self {
            size,
            entering: (0..size).map(|_| AtomicBool::new(false)).collect(),
            tickets: (0..size).map(|_| AtomicUsize::new(0)).collect(),
        }
    }
    pub fn lock(&self, thread_idx: usize) -> LockGuard<'_> {
        let ticket = self.issue_ticket(thread_idx);
        self.enter_critical_section(ticket, thread_idx)
    }

    fn issue_ticket(&self, thread_idx: usize) -> usize {
        self.entering[thread_idx].store(true, Ordering::Release);

        // Current ticket = max(prev_ticket) + 1
        let mut prev_ticket = 0;
        for i in 0..self.size {
            let ticket = self.tickets[i].load(Ordering::Acquire);
            prev_ticket = prev_ticket.max(ticket);
        }
        let ticket = prev_ticket + 1;
        self.entering[thread_idx].store(false, Ordering::Release);

        ticket
    }

    fn enter_critical_section(&self, ticket: usize, thread_idx: usize) -> LockGuard<'_> {
        for other_thread_idx in 0..self.size {
            // Skip the current thread
            if other_thread_idx == thread_idx {
                continue;
            }

            // Check other thread's state
            // 1. Entering the State -> Wait
            while self.entering[other_thread_idx].load(Ordering::Acquire) {
                println!(
                    "Thread {} is waiting for thread {}",
                    thread_idx, other_thread_idx
                );
            }

            loop {
                match self.tickets[other_thread_idx].load(Ordering::Acquire) {
                    0 => {
                        break;
                    }
                    other_ticket => {
                        if ticket < other_ticket
                            || ticket == other_ticket && thread_idx < other_thread_idx
                        {
                            break;
                        }
                    }
                }
            }
        }

        LockGuard {
            lock: self,
            thread_idx,
        }
    }
}

struct LockGuard<'a> {
    lock: &'a BakeryLock,
    thread_idx: usize,
}

impl<'a> Drop for LockGuard<'a> {
    fn drop(&mut self) {
        self.lock.tickets[self.thread_idx].store(0, Ordering::Release);
    }
}

pub fn run() {
    const NUM_THREADS: usize = 4;
    const NUM_LOOP: usize = 10;

    let lock = Arc::new(BakeryLock::new(NUM_THREADS));
    let counter = Arc::new(AtomicUsize::new(0));

    let mut v = Vec::new();
    for thread_idx in 0..NUM_THREADS {
        let lock = Arc::clone(&lock);
        let counter = Arc::clone(&counter);
        let thread = std::thread::spawn(move || {
            for _ in 0..NUM_LOOP {
                let _ = lock.lock(thread_idx);
                let n = counter.fetch_add(1, Ordering::Relaxed);
                println!("Thread {} incremented counter to {}", thread_idx, n);
            }
        });
        v.push(thread);
    }

    for thread in v {
        thread.join().unwrap();
    }
    let got = counter.load(Ordering::Relaxed);
    assert_eq!(got, NUM_THREADS * NUM_LOOP);
    println!("OK: {}", got);
}
