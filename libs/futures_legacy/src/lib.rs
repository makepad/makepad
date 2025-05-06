pub mod executor;
pub mod task;
/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        use {futures_timer::Delay, std::time::Duration};

        executor::block_on(async {
            Delay::new(Duration::from_secs(1)).await;
            println!("ONE");
            Delay::new(Duration::from_secs(1)).await;
            println!("TWO");
            Delay::new(Duration::from_secs(1)).await;
            println!("THREE");
        });
        println!("DONE");
    }
}*/
