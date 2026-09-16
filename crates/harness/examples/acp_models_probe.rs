//! Live probe: real grok/hermes model discovery through the harness path.
use zeron_harness::{AcpHarness, Harness};

#[tokio::main]
async fn main() {
    for (name, models) in [
        ("grok", AcpHarness::grok().models().await),
        ("hermes", AcpHarness::hermes().models().await),
    ] {
        match models {
            Ok(models) => {
                println!("{name}: count={}", models.len());
                for m in models.iter().take(8) {
                    println!("  {} | {} | ladder={:?}", m.id, m.label, m.reasoning_levels);
                }
            }
            Err(e) => println!("{name}: ERR {e}"),
        }
    }
}
