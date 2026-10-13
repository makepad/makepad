// mpsc under load: unbounded, bounded and rendezvous channels with several senders; the sums,
// counts and orders printed must equal real std's.
use std::sync::mpsc;
use std::thread;

fn run_senders(tx: mpsc::Sender<u64>, senders: u64, per: u64) -> Vec<thread::JoinHandle<()>> {
    let mut hs = Vec::new();
    for s in 0..senders {
        let tx = tx.clone();
        hs.push(thread::spawn(move || {
            for i in 0..per {
                tx.send(s * 1_000_000 + i).unwrap();
            }
        }));
    }
    drop(tx);
    hs
}

#[test]
fn a01_unbounded_ping() {
    let (tx, rx) = mpsc::channel::<u64>();
    let h = thread::spawn(move || {
        for i in 0..200_000u64 {
            tx.send(i).unwrap();
        }
    });
    let mut sum = 0u64;
    let mut ordered = true;
    let mut last = 0u64;
    for k in 0..200_000u64 {
        let v = rx.recv().unwrap();
        if k > 0 && v != last + 1 {
            ordered = false;
        }
        last = v;
        sum += v;
    }
    h.join().unwrap();
    println!("| unbounded sum {} ordered {} then {:?}", sum, ordered, rx.recv());
}

#[test]
fn a02_many_senders() {
    let (tx, rx) = mpsc::channel::<u64>();
    let hs = run_senders(tx, 4, 50_000);
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut per_sender_ordered = true;
    let mut last = [u64::MAX; 4];
    for v in rx.iter() {
        let s = (v / 1_000_000) as usize;
        let i = v % 1_000_000;
        if last[s] != u64::MAX && i != last[s] + 1 {
            per_sender_ordered = false;
        }
        last[s] = i;
        sum += v;
        n += 1;
    }
    for h in hs {
        h.join().unwrap();
    }
    println!("| many n {} sum {} per-sender ordered {}", n, sum, per_sender_ordered);
}

#[test]
fn a03_bounded() {
    for cap in [1usize, 2, 16] {
        let (tx, rx) = mpsc::sync_channel::<u64>(cap);
        let h = thread::spawn(move || {
            for i in 0..50_000u64 {
                tx.send(i).unwrap();
            }
        });
        let mut sum = 0u64;
        while let Ok(v) = rx.recv() {
            sum += v;
        }
        h.join().unwrap();
        println!("| bounded cap {} sum {}", cap, sum);
    }
}

#[test]
fn a04_rendezvous() {
    let (tx, rx) = mpsc::sync_channel::<u64>(0);
    let (back_tx, back_rx) = mpsc::channel::<u64>();
    let h = thread::spawn(move || {
        for i in 0..5_000u64 {
            tx.send(i).unwrap();
            back_tx.send(i).unwrap();
        }
    });
    let mut sum = 0u64;
    for _ in 0..5_000 {
        sum += rx.recv().unwrap();
        sum += back_rx.recv().unwrap();
    }
    h.join().unwrap();
    println!("| rendezvous sum {} after {:?} {:?}", sum, rx.try_recv(), back_rx.try_recv());
}

#[test]
fn a05_try_and_timeout() {
    let (tx, rx) = mpsc::sync_channel::<u32>(1);
    println!("| try_send {:?} {:?}", tx.try_send(1), tx.try_send(2));
    println!("| try_recv {:?} {:?}", rx.try_recv(), rx.try_recv());
    println!("| timeout {:?}", rx.recv_timeout(std::time::Duration::from_millis(5)));
    drop(tx);
    println!("| disconnected {:?} {:?}", rx.try_recv(), rx.recv_timeout(std::time::Duration::from_millis(5)));
}
