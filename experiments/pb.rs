use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let task_count: usize = args[1].parse().expect("Not a number");
    let mut set = JoinSet::new();

    for i in 0..task_count {
        set.spawn(async move {
            sleep(Duration::from_secs(30)).await;
            format!("Task {i} complete")
        });
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(value) => println!("{value}"),
            Err(e) => eprintln!("Task panicked: {e}"),
        }
    }
}
