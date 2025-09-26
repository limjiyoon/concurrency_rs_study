use crate::lock::semaphore::Semaphore;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[derive(Clone)]
struct Sender<T> {
    semaphore: Arc<Semaphore>,
    buffer: Arc<Mutex<VecDeque<T>>>,
    cond: Arc<Condvar>,
}

impl<T: Send> Sender<T> {
    fn send(&self, data: T) {
        self.semaphore.wait();
        let mut buffer = self.buffer.lock().unwrap();
        buffer.push_back(data);
        self.cond.notify_one();
    }
}

struct Receiver<T> {
    semaphore: Arc<Semaphore>,
    buffer: Arc<Mutex<VecDeque<T>>>,
    cond: Arc<Condvar>,
}

impl<T> Receiver<T> {
    fn recv(&self) -> T {
        let mut buffer = self.buffer.lock().unwrap();
        loop {
            if let Some(data) = buffer.pop_front() {
                self.semaphore.post();
                return data;
            }
            buffer = self.cond.wait(buffer).unwrap();
        }
    }
}

fn channel<T>(max: isize) -> (Sender<T>, Receiver<T>) {
    assert!(max > 0);
    let semaphore = Arc::new(Semaphore::new(max));
    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    let cond = Arc::new(Condvar::new());

    let tx = Sender {
        semaphore: semaphore.clone(),
        buffer: buffer.clone(),
        cond: cond.clone(),
    };

    let rx = Receiver {
        semaphore: semaphore.clone(),
        buffer: buffer.clone(),
        cond: cond.clone(),
    };

    (tx, rx)
}

pub fn run() {
    const NUM_LOOP: isize = 1000;
    const NUM_THREADS: isize = 4;
    const BUFFER_SIZE: isize = 4;

    let (tx, rx) = channel(BUFFER_SIZE);
    let mut v = Vec::new();

    let read_thread = thread::spawn(move || {
        let mut cnt = 0;
        while cnt < NUM_LOOP * NUM_THREADS {
            let n = rx.recv();
            println!("Received n: {:?}", n);
            cnt += 1;
        }
    });
    v.push(read_thread);

    for thread_idx in 0..NUM_THREADS {
        let cur_tx = tx.clone();
        let write_thread = thread::spawn(move || {
            for loop_idx in 0..NUM_LOOP {
                cur_tx.send((thread_idx, loop_idx));
            }
        });
        v.push(write_thread);
    }

    for thread in v {
        thread.join().unwrap();
    }
}
