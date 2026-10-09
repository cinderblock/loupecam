//! Experiment: stop a stream and start again after a delay.
//! Usage: cargo run --example restart -- <delay_ms> [reopen]
use loupecam::{Camera, PixelMode, StreamConfig};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_default()).init();
    let delay: u64 = std::env::args().nth(1).map_or(Ok(500), |s| s.parse())?;
    let mut cam = Camera::open_first()?;
    let cfg = StreamConfig::new(2, PixelMode::Raw8);
    let rx = cam.start(&cfg)?;
    rx.recv_timeout(Duration::from_secs(3))?;
    cam.stop()?;
    std::thread::sleep(Duration::from_millis(delay));
    let t = Instant::now();
    match cam.start(&cfg) {
        Ok(rx) => {
            let f = rx.recv_timeout(Duration::from_secs(3))?;
            println!("delay {delay} ms: restart OK in {:?}, frame seq {}", t.elapsed(), f.trailer.sequence);
        }
        Err(e) => println!("delay {delay} ms: restart failed: {e}"),
    }
    Ok(())
}
