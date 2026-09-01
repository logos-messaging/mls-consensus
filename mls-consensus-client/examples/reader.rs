//! Reads a group's state. No wallet, no keys.
//!
//! ```bash
//! cargo run -p mls-consensus-client --example reader -- <ACCOUNT_ID>
//! ```

use mls_consensus_client::{AccountId, ConvoStateReader};

const SEQUENCER: &str = "http://localhost:3040";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let account: AccountId = std::env::args()
        .nth(1)
        .ok_or("usage: reader <ACCOUNT_ID>")?
        .parse()?;

    match ConvoStateReader::connect(SEQUENCER, account)?.get().await? {
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
