//! End-to-end: create a wallet, initialize a group, commit to it, lose a race.
//!
//! Needs a local sequencer: see [LEZ](https://github.com/logos-blockchain/logos-execution-zone)

use mls_consensus_client::{ConvoStateReader, ConvoStateWriter};

const SEQUENCER: &str = "http://localhost:3040";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The writer's wallet is ephemeral, so this never touches ~/.lee/wallet.
    let writer = ConvoStateWriter::new(SEQUENCER).await?;

    // Deploying an already-deployed program is a no-op: its identity is the hash of its bytecode.
    writer.deploy().await?;
    println!("  program deployed");

    let account = writer.account();
    println!("  group:    {account}");

    // `initialize` only submits, so wait for the group to exist before committing to it.
    writer.initialize().await?;
    let reader = ConvoStateReader::connect(SEQUENCER, account)?;
    reader.await_version(0).await?;
    println!("  initialized at v0");

    // `commit` settles before it returns, so `Ok` means this epoch is the one on chain.
    writer.commit(0, [7; 32].into()).await?;

    let state = reader.get().await?.expect("just committed");
    println!(
        "\nafter commit: v{} — {} epoch(s), head {}",
        state.version,
        state.epoch_history.len(),
        hex(state
            .epoch_history
            .last()
            .copied()
            .expect("history_exists")
            .as_bytes()),
    );

    // A member still holding v0 loses the race, loudly.
    match writer.commit(0, [9; 32].into()).await {
        Err(error) => println!("stale commit refused: {error}"),
        Ok(()) => panic!("a stale commit should not have succeeded"),
    }

    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
