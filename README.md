# mls-consensus

Consensus for MLS group state on the [Logos Execution Zone](https://github.com/logos-blockchain/logos-execution-zone).

This is a POC which provides consensus on which commits should be applied to an MLS group.


## How it works

### Creator

- Creator of the group also creates a wallet, and initializes program state.
- The creator shares the group's `AccountId` with any other admins in the group.
- When admins generate a commit, they first push a hash of the resulting group context — an `EpochFingerprint` — to the chain prior to merging.
- If the transaction is successful, then the group state can be merged safely as there were no competing commits.
- If the transaction is not successful then the state was updated by someone else. The generated commit would break the group and must be dropped.

### Members

- Any Member receiving a StagedCommit, must first check the group's epoch history, to see if the commit should be applied.
- The member verifies that the fingerprint of the resultant group context matches the corresponding entry in `ConvoState::epoch_history`.
- If match then this commit should be merged
- If doesn't match then this commit would cause a fork and must be dropped.


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

## Try it against the shared chain

There is a running chain you can use without setting anything up. All four examples default to it
and honour `SEQUENCER_URL`.

```bash
cargo run -p mls-consensus-client --example poc       # create a group, commit, lose a race
cargo run -p mls-consensus-client --example epochs    # create a group, walk it through 3 epochs
```

Both print the group's `AccountId`, which the read-only examples take:

```bash
cargo run -p mls-consensus-client --example reader -- <ACCOUNT_ID>
cargo run -p mls-consensus-client --features blocking --example blocking_read -- <ACCOUNT_ID>
```

Each commit waits for a block (15s slots), so `epochs` takes a minute or two.

**Block explorer: <http://68.183.19.197:8080>** — browse blocks, transactions and accounts.

If a reader reports "holds no group state" for an account you just created, check you are pointing
at the same chain that created it. A reader aimed at a *different* sequencer connects fine and
truthfully reports no state, which looks identical to a bug.

## The shared chain

| | |
| --- | --- |
| Sequencer RPC | `http://68.183.19.197:3040` — JSON-RPC, public, no auth |
| Explorer | `http://68.183.19.197:8080` |

Four layers: a client talks to the **sequencer**, which executes the program and publishes every
block to a single-validator **Bedrock** node (private, not any public Logos network) for finality;
an **indexer** reads that channel back; the **explorer** serves the UI from the indexer. Everything
is built from the revision this workspace pins, so it speaks the same account encoding as the
program.

The RPC is open on purpose — anyone can create a group and read any group. That is safe for the
program by construction: a program's identity is the hash of its bytecode, `initialize` only claims
uninitialised accounts, writes require the account's own signature, and a program can only write
accounts it already owns. So a stranger's program cannot touch your conversations. What an open
endpoint costs is disk and CPU.

Health:

```bash
ssh root@68.183.19.197 lez-health     # exits non-zero if the stack is wedged
```

Run that if anything looks wrong. It compares the sequencer's height against the indexer's, which is
the only reliable signal — there is a known failure where the sequencer keeps producing blocks and
logging success while Bedrock has stopped accepting them, and no single service reports a fault.
**Restarting the sequencer can trigger it, and a restart does not recover it.**

Operator details — systemd units, the genesis ceremony, key rotation, recovery procedures and the
traps — are in `DEPLOYMENT.md`, which is deliberately not in git because it covers host specifics.

## Running your own sequencer

For offline work, run one from the *pinned* upstream revision — the copy cargo already fetched —
rather than a hand-cloned checkout, which is one `git checkout` away from silently disagreeing with
`Cargo.toml`. Upstream ships a `Justfile`:

```bash
cd "$(find ~/.cargo/git/checkouts -maxdepth 2 -type d -name a98ca4a -path '*logos-execution-zone*')"
RISC0_DEV_MODE=1 just run-sequencer-standalone --listen-address 127.0.0.1 --home ~/.lez-testing

SEQUENCER_URL=http://localhost:3040 cargo run -p mls-consensus-client --example poc
```

If `a98ca4a` no longer matches the `rev` pinned in `Cargo.toml`, use the new one.

Each piece earns its place:

- **`standalone`** swaps in a mock block publisher, so nothing needs a Bedrock node and there is no
  Docker. The executor, RocksDB and the RPC all still run for real — transactions are ordered,
  executed and applied; they just are not anchored to a DA layer. **This also means no explorer**,
  since the explorer reads the indexer and the indexer reads Bedrock.
- **`RISC0_DEV_MODE=1`** skips proof verification, which is what makes iteration bearable. The
  `Justfile` does not set it for you. Never set it for anything you mean to trust.
- **`--listen-address 127.0.0.1`** overrides a default of `0.0.0.0`. The RPC has no caller auth, so
  the default exposes an unauthenticated transaction endpoint to the whole network.
- **`--home`** keeps RocksDB and `bedrock_signing_key` out of the cargo checkout. The debug config
  says `"home": "."`, so without this the chain state is written into cargo's git cache — which
  cargo prunes whenever it likes.

Check it is actually working — a passing `checkHealth` only proves the RPC is up, so confirm
`getLastBlockId` *moves*:

```bash
for m in checkHealth getLastBlockId; do
  curl -s -X POST http://localhost:3040 -H 'content-type: application/json' \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$m\",\"params\":[]}"; echo
done
```

A sequencer that answers health checks but never produces a block hangs `await_version` with no
error — rule that out before blaming your own code. Bootstrap takes about a minute, so sampling too
early looks the same as broken. Metrics are on `:9000`.

When it misbehaves it is nearly always stale state: `rm -rf ~/.lez-testing`. Shutdown holds the
store open for a while after the process exits, so allow a few seconds before restarting or it will
find its own home directory locked.

## What is POC-shaped about this

- **Wallets are ephemeral.** `ConvoStateWriter::new` mints keys into a tempdir and drops them, so
  a group created in one process can never be committed to by another. That is the whole account
  model for now.
- **Account keys cannot be rotated.** An `AccountId` *is* a domain-separated SHA-256 of the public
  key, and authorisation re-derives it from the signer, so a different key is a different account.
  There is no authority field and no rotation instruction anywhere in LEE. A leaked group key means
  migrating to a new group, which the program has no notion of.
- **History is unbounded.** `ConvoState::epoch_history` grows one fingerprint per commit forever.
- **`ConvoState::validate` is a stub** — no invariants are enforced yet.
- **The client's own logic is untested.** `core` and `program` are well covered, but every client
  test is in `blocking.rs`; `classify()` — which decides whether your commit landed or was
  superseded — has no tests at all.
- **The upstream pin is deliberate.** A program is bound to the protocol revision it was compiled
  against, and the client must stay on the same revision as the program. Bump all five
  `logos-execution-zone` deps together, or none.
