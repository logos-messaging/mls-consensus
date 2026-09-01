//! The compiled MLS consensus guest program.
//!
//! `build.rs` cross-compiles `guest/` for the zkVM and generates this module's contents: one set
//! of constants per guest binary, named after that binary. `guest/src/bin/convo_state.rs`
//! therefore yields
//!
//! * `CONVO_STATE_ELF: &[u8]` — the RISC-V bytecode the node executes,
//! * `CONVO_STATE_ID: [u32; 8]` — its image ID, which *is* the program's on-chain identity,
//! * `CONVO_STATE_PATH: &str` — where the ELF was written, for tooling that wants the file.
//!
//! Renaming the guest binary renames these constants, so the name is part of this crate's public
//! API even though nothing in this file mentions it.
//!
//! The image ID is a hash of the bytecode, so changing a single line of the program changes its
//! ID and therefore makes it a different program. Accounts stay bound to the exact program that
//! claimed them: a group under one image ID cannot be committed to by another, which is what
//! stops a rebuilt program from quietly taking over an existing conversation.

include!(concat!(env!("OUT_DIR"), "/methods.rs"));
