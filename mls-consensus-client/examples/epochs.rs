//! Creates a group and walks it through several epochs, printing the chain as it grows.
//!
//! ```bash
//! cargo run -p mls-consensus-client --example epochs
//! SEQUENCER_URL=http://1.2.3.4:3040 cargo run -p mls-consensus-client --example epochs
//! ```
//!
//! Each commit waits for a block, so this takes a minute or two.

use mls_consensus_client::{ConvoStateReader, ConvoStateWriter, EpochFingerprint};

/// How many epochs to advance through after initializing.
const EPOCHS: u64 = 3;

/// The sequencer to talk to.
fn sequencer() -> String {
    std::env::var("SEQUENCER_URL").unwrap_or_else(|_| "http://68.183.19.197:3040".to_owned())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sequencer = sequencer();

    // Creating the writer *is* creating the account: the wallet derives one and the
    // group will live in it. The keys are ephemeral, so this group can only ever be
    // committed to by this process.
    let writer = ConvoStateWriter::new(&sequencer).await?;
    let account = writer.account();

    // A no-op if the program is already on this zone — its identity is the hash of
    // its bytecode, so deploying the same bytes twice changes nothing.
    writer.deploy().await?;

    println!("group {account}\n");

    // `initialize` only submits the transaction, so wait for the account to actually
    // hold a group before committing to it.
    writer.initialize().await?;
    let reader = ConvoStateReader::connect(&sequencer, account)?;
    reader.await_version(0).await?;
    println!("  v0  initialized, no epochs recorded");

    // One commit per epoch. `expected_version` is the compare-and-swap: it names the
    // version this commit replaces, so if another member had committed first this
    // would fail rather than clobber them. `commit` settles before returning, so
    // after it the group really is at `expected + 1`.
    for epoch in 1..=EPOCHS {
        let expected_version = epoch - 1;
        let fingerprint = fingerprint_for(epoch);

        writer.commit(expected_version, fingerprint).await?;

        println!(
            "  v{epoch}  committed {}",
            short(&hex(fingerprint.as_bytes()))
        );
    }

    // The account is the whole story — a reader needs nothing else to reconstruct it.
    let state = reader
        .get()
        .await?
        .expect("the group was just committed to");

    println!("\nepoch history ({} epochs, oldest first):", state.version);
    for (index, fingerprint) in state.epoch_history.iter().enumerate() {
        let marker = if index + 1 == state.epoch_history.len() {
            "<- current"
        } else {
            ""
        };
        println!("  {index:>3}  {} {marker}", hex(fingerprint.as_bytes()));
    }

    // Carry the sequencer through explicitly: a bare `reader` invocation would
    // use its own default, which may be a different chain entirely.
    println!("\nread it back with:");
    println!(
        "  SEQUENCER_URL={sequencer} cargo run -p mls-consensus-client --example reader -- {account}"
    );

    Ok(())
}

/// Stands in for a real epoch fingerprint.
///
/// A member would hash its own MLS `GroupContext` after applying the commit — the
/// epoch number, tree hash and confirmed transcript hash — so that members who
/// applied the same commits in the same order produce the same 32 bytes. The
/// program never reads these bytes, so for a demo any distinguishable value does.
fn fingerprint_for(epoch: u64) -> EpochFingerprint {
    let mut bytes = [0u8; 32];
    bytes[0] = 0xEE;
    bytes[31] = epoch as u8;
    bytes.into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// First and last four bytes, for a line that has to stay readable.
fn short(hex: &str) -> String {
    format!("{}…{}", &hex[..8], &hex[hex.len() - 8..])
}
