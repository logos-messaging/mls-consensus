//! Reading from synchronous code — no `#[tokio::main]`, no runtime of the caller's own.
//!
//! ```bash
//! cargo run -p mls-consensus-client --features blocking --example blocking_read -- <ACCOUNT_ID>
//! ```

use mls_consensus_client::{AccountId, blocking::BlockingReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let account: AccountId = std::env::args()
        .nth(1)
        .ok_or("usage: blocking_read <ACCOUNT_ID>")?
        .parse()?;

    // Note the absence of an `.await`, an executor, or a `block_on` at this call site: that is
    // the whole point of the wrapper.
    let reader = BlockingReader::connect(&sequencer(), account)?;

    match reader.get()? {
        None => println!("{account} holds no group state"),
        Some(state) => {
            println!("account: {account}");
            println!("version: {}", state.version);
            println!("epochs: {}", state.epoch_history.len());
        }
    }

    Ok(())
}

/// The sequencer to talk to. Override to point at a remote one:
/// `SEQUENCER_URL=http://1.2.3.4:3040 cargo run -p mls-consensus-client --example blocking_read`
fn sequencer() -> String {
    std::env::var("SEQUENCER_URL").unwrap_or_else(|_| "http://68.183.19.197:3040".to_owned())
}
