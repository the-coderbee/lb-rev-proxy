use std::thread;
use std::time::Duration;

fn main() {
    let thread_count: usize = std::env::args()
        .nth(1)
        .expect("usage: pa <thread_count>")
        .parse()
        .expect("thread_count must be a non-negative integer");
    let mut handles = Vec::with_capacity(thread_count);

    for _ in 0..=thread_count {
        let result = thread::Builder::new().spawn(move || thread::sleep(Duration::from_secs(30)));

        match result {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                println!("handles len: {}", handles.len());
                eprintln!("{}", error);
                break;
            }
        }
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("All threads have finished execution.");
}
