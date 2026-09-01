//! The MLS consensus guest program.
//!
//! This is the whole on-chain entry point. It is intentionally a shim: it reads the LEE inputs,
//! hands them to `mls_consensus_program::execute`, and commits the proposed transition. All the
//! interesting logic lives in `mls_consensus_program`, where it can be tested on the host without
//! a zkVM in the loop.
//!
//! This file's name matters: risc0 names the constants in `mls-consensus-program/methods` after
//! the guest binary, so renaming it renames `CONVO_STATE_ELF` and `CONVO_STATE_ID`.
//!
//! The same bytecode serves both execution modes. For a public account the node runs it
//! directly; for a private account the client runs it locally and proves the run in zero
//! knowledge. Nothing here is aware of the difference — and neither mode learns anything about
//! the conversation, since an epoch is only ever an opaque fingerprint to the program.

use lee_core::program::{ProgramInput, ProgramOutput, read_lee_inputs};
use mls_consensus_core::Instruction;

fn main() {
    let (
        ProgramInput {
            self_program_id,
            caller_program_id,
            pre_states,
            instruction,
        },
        instruction_data,
    ) = read_lee_inputs::<Instruction>();

    // LEE compares the pre-states the program echoes against the accounts it was actually
    // given, so keep the originals intact and hand `execute` a clone.
    let post_states =
        mls_consensus_program::execute(self_program_id, pre_states.clone(), instruction);

    // Constructing a `ProgramOutput` has no effect on its own: `.write()` is what commits it
    // to the journal.
    ProgramOutput::new(
        self_program_id,
        caller_program_id,
        instruction_data,
        pre_states,
        post_states,
    )
    .write();
}
