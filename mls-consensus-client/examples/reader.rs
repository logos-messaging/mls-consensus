//! Reads a group's state. No wallet, no keys.
//!
//! ```bash
//! cargo run -p mls-consensus-client --example reader -- <ACCOUNT_ID>
//! ```

use mls_consensus_client::{AccountId, ConvoStateReader};

/// The sequencer to talk to. Override to point at a remote one:
/// `SEQUENCER_URL=http://1.2.3.4:3040 cargo run -p mls-consensus-client --example reader`
fn sequencer() -> String {
    std::env::var("SEQUENCER_URL").unwrap_or_else(|_| "http://68.183.19.197:3040".to_owned())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let account: AccountId = std::env::args()
        .nth(1)
        .ok_or("usage: reader <ACCOUNT_ID>")?
        .parse()?;

    match ConvoStateReader::connect(&sequencer(), account)?
        .get()
        .await?
    {
        None => println!("{account} holds no group state"),
        Some(state) => {
            println!("account: {account}");
            println!("version: {}", state.version);
            println!("epochs: {}", state.epoch_history.len());
            for (index, fingerprint) in state.epoch_history.iter().enumerate() {
                println!("    {index:>3}  {}", hex(fingerprint.as_bytes()));
            }
        }
    }

    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
