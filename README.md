# mls-consensus

Consensus for MLS group state on the [Logos Execution Zone](https://github.com/logos-blockchain/logos-execution-zone).

This is a POC which provides consensus on which commits should be applied to an MLS group.


## How it works

### Creator

- Creator of the group also creates a wallet, and initialized program state.
- The creator shares the wallet address with any other admins in the group.
- When admins generate a commit, they first push a hash of the resulting group context to the chain prior to merging.
- If the transaction is successful, then the group state can be merged safely as there were no completing commits.
- If the transaction is not successful then the state was updated by someone else. The generated commit would break the group and must be dropped.

### Members

- Any Member receiving a StagedCommit, must first check the commit log, to see if the commit should be applied.
- The member verifies that the hash of the resultant group context matches the entry in the commitLog.
- If match then this commit should be merged
- If doesn't match then this commit would cause a work and must be dropped.


## Layout

| Crate | What it is |
| --- | --- |
| `mls-consensus-program/core` | The wire format: `ConvoState`, `Instruction`. No logic, no zkVM. |
| `mls-consensus-program/program` | The state transitions, as plain host-testable functions. |
| `mls-consensus-program/methods` | Cross-compiles `guest/` for the zkVM; exports the ELF and image ID. |
| `mls-consensus-client` | Async `ConvoStateReader` / `ConvoStateWriter`, plus a `blocking` feature. |

## Prerequisites

- **Rust** — pinned by `rust-toolchain.toml` (1.94.0); rustup will fetch it.
- **RISC Zero toolchain** — the guest is cross-compiled to `riscv32im-risc0-zkvm-elf`:
  ```bash
  curl -L https://risczero.com/install | bash && rzup install
  ```

## Build and test

```bash
cargo test --workspace          # host tests: wire format, transitions, guest-in-zkVM
```

The `methods` tests run the guest under the zkVM, so the first build is slow.

## Run a local sequencer

The examples expect a sequencer at `http://localhost:3040`. Run one from the *pinned* upstream
revision — the copy cargo already fetched — rather than a hand-cloned checkout, which is one
`git checkout` away from silently disagreeing with `Cargo.toml`:

```bash
cd "$(find ~/.cargo/git/checkouts -maxdepth 2 -type d -name a98ca4a -path '*logos-execution-zone*')/lez/sequencer/service"
RISC0_DEV_MODE=1 cargo run --release --features standalone -p sequencer_service -- \
    configs/debug/sequencer_config.json
```

`standalone` mocks the Bedrock node, so there is no Docker to run. `RISC0_DEV_MODE=1` skips proof
verification, which is what makes iteration bearable — never set it for anything you mean to trust.
If `a98ca4a` no longer matches the `rev` pinned in `Cargo.toml`, use the new one.

## Run the POC

With the sequencer up, in another shell:

```bash
cargo run -p mls-consensus-client --example poc
```

That deploys the program, creates a group, commits an epoch, then loses a deliberate race to show
the stale commit being refused. It prints the group's `AccountId`, which the read-only examples take:

```bash
cargo run -p mls-consensus-client --example reader -- <ACCOUNT_ID>
cargo run -p mls-consensus-client --features blocking --example blocking_read -- <ACCOUNT_ID>
```

## What is POC-shaped about this

- **Wallets are ephemeral.** `ConvoStateWriter::new` mints keys into a tempdir and drops them, so
  a group created in one process can never be committed to by another. That is the whole account
  model for now.
- **History is unbounded.** `ConvoState::epoch_history` grows one fingerprint per commit forever.
- **`ConvoState::validate` is a stub** — no invariants are enforced yet.
- **The upstream pin is deliberate.** A program is bound to the protocol revision it was compiled
  against, and the client must stay on the same revision as the program. Bump all five
  `logos-execution-zone` deps together, or none.
