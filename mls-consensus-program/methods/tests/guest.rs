//! End-to-end tests that run the *compiled* guest program inside the zkVM.
//!
//! The unit tests in `mls_consensus_program` cover the transition logic. These cover the things
//! only the real ELF can tell us: that the guest is wired up correctly, that its journal decodes
//! as a `ProgramOutput`, and that the transitions it proposes are ones LEE will actually accept.
//!
//! They execute the program without proving it, which is what a node does for a public account.
//! Proving is orders of magnitude slower and exercises no additional program logic.

use std::borrow::Cow;

use lee::program::Program;
use lee_core::{
    account::{Account, AccountId, AccountWithMetadata, Data, Nonce},
    program::{AccountPostState, Claim, ProgramOutput, validate_execution},
};
use mls_consensus_core::{ConvoState, Instruction};
use mls_consensus_methods::{CONVO_STATE_ELF, CONVO_STATE_ID};
use risc0_zkvm::{ExecutorEnv, default_executor};

const ACCOUNT_ID: AccountId = AccountId::new([3; 32]);

fn program() -> Program {
    Program::new(Cow::Borrowed(CONVO_STATE_ELF)).expect("the embedded ELF should be valid")
}

/// Runs the guest over `pre_states` and returns the output it committed to its journal.
///
/// Mirrors what `lee`'s own public-execution path does: write a `ProgramInput` frame, execute,
/// then decode the journal frame back into a `ProgramOutput`.
fn run(
    pre_states: &[AccountWithMetadata],
    instruction: Instruction,
) -> Result<ProgramOutput, String> {
    let program = program();
    let instruction_data =
        Program::serialize_instruction(instruction).expect("instruction should encode");

    let mut env_builder = ExecutorEnv::builder();
    program
        .write_inputs(None, pre_states, &instruction_data, &mut env_builder)
        .expect("inputs should encode");

    let session = default_executor()
        .execute(env_builder.build().unwrap(), program.elf())
        .map_err(|error| error.to_string())?;

    let payload = lee_core::from_frame(&session.journal.bytes)
        .expect("the journal should be a well-formed frame");

    Ok(borsh::from_slice(payload).expect("the journal should decode as a ProgramOutput"))
}

/// Runs the guest and asserts that LEE would accept the transition it proposes.
fn run_accepted(pre_states: &[AccountWithMetadata], instruction: Instruction) -> ProgramOutput {
    let output = run(pre_states, instruction).expect("the guest should not reject this");

    validate_execution(&output.pre_states, &output.post_states, program().id())
        .expect("LEE should accept the proposed transition");

    output
}

/// Applies a post-state the way the runtime would, so a test can chain several instructions.
///
/// The only part the program does not do itself is the ownership change: a program requests a
/// claim, and the runtime is what actually sets `program_owner`.
fn settle(post_state: &AccountPostState) -> AccountWithMetadata {
    let mut account = post_state.account().clone();

    if post_state.required_claim() == Some(Claim::Authorized) {
        account.program_owner = AccountId::from(program().id());
    }

    AccountWithMetadata {
        account,
        is_authorized: true,
        account_id: ACCOUNT_ID,
    }
}

fn uninitialized() -> AccountWithMetadata {
    AccountWithMetadata {
        account: Account::default(),
        is_authorized: true,
        account_id: ACCOUNT_ID,
    }
}

fn state_in(post_state: &AccountPostState) -> ConvoState {
    ConvoState::try_from(&post_state.account().data).expect("post state should hold a ConvoState")
}

fn commit_at(expected_version: u64, epoch_fingerprint: [u8; 32]) -> Instruction {
    Instruction::Commit {
        expected_version,
        epoch_fingerprint: epoch_fingerprint.into(),
    }
}

#[test]
fn the_embedded_image_id_is_the_program_id() {
    // The image ID generated at build time and the ID `lee` computes from the bytecode must
    // agree, otherwise transactions would name a program the node cannot find.
    assert_eq!(program().id(), CONVO_STATE_ID);
}

#[test]
fn initializing_claims_the_account_and_starts_an_empty_group() {
    let output = run_accepted(&[uninitialized()], Instruction::Initialize {});

    let [post_state] = <[AccountPostState; 1]>::try_from(output.post_states).unwrap();
    assert_eq!(post_state.required_claim(), Some(Claim::Authorized));

    let state = state_in(&post_state);
    assert_eq!(state.version, 0);
    assert!(state.epoch_history.is_empty());
}

#[test]
fn the_guest_echoes_its_instruction_and_pre_states() {
    // LEE binds a transaction to the instruction it was built from by comparing these, so the
    // guest must return them untouched.
    let instruction = Instruction::Initialize {};
    let expected_data = Program::serialize_instruction(instruction.clone()).unwrap();
    let pre_states = [uninitialized()];

    let output = run_accepted(&pre_states, instruction);

    assert_eq!(output.instruction_data, expected_data);
    assert_eq!(output.pre_states, pre_states.to_vec());
    assert_eq!(output.self_program_id, CONVO_STATE_ID);
    assert_eq!(output.caller_program_id, None);
}

#[test]
fn a_group_advances_through_a_chain_of_commits() {
    let initialized = run_accepted(&[uninitialized()], Instruction::Initialize {});
    let account = settle(&initialized.post_states[0]);

    let first = run_accepted(&[account], commit_at(0, [1; 32]));
    assert_eq!(state_in(&first.post_states[0]).version, 1);
    let account = settle(&first.post_states[0]);

    let second = run_accepted(&[account], commit_at(1, [2; 32]));

    let state = state_in(&second.post_states[0]);
    assert_eq!(state.version, 2);
    assert_eq!(state.epoch_history, vec![[1; 32].into(), [2; 32].into()]);
}

#[test]
fn a_commit_against_a_stale_version_is_rejected_by_the_guest() {
    let initialized = run_accepted(&[uninitialized()], Instruction::Initialize {});
    let account = settle(&initialized.post_states[0]);

    let committed = run_accepted(&[account], commit_at(0, [1; 32]));
    let account = settle(&committed.post_states[0]);

    // A member still holding version 0 now loses the race, loudly.
    let error =
        run(&[account], commit_at(0, [9; 32])).expect_err("the guest should reject a stale commit");

    assert!(
        error.contains("version conflict"),
        "expected a version conflict, got: {error}"
    );
}

#[test]
fn an_unauthorized_instruction_is_rejected_by_the_guest() {
    // An account nobody signed for: the node would set `is_authorized == false`.
    let unauthorized = AccountWithMetadata {
        is_authorized: false,
        ..uninitialized()
    };

    let error = run(&[unauthorized], Instruction::Initialize {})
        .expect_err("the guest should reject an unauthorized instruction");

    assert!(
        error.contains("must authorize"),
        "expected an authorization failure, got: {error}"
    );
}

#[test]
fn a_group_account_owned_by_another_program_is_rejected() {
    let foreign = AccountWithMetadata {
        account: Account {
            program_owner: AccountId::from([9_u32; 8]),
            balance: 0,
            data: Data::try_from(&ConvoState::new()).unwrap(),
            nonce: Nonce::from(0_u128),
        },
        is_authorized: true,
        account_id: ACCOUNT_ID,
    };

    let error = run(&[foreign], commit_at(0, [1; 32]))
        .expect_err("the guest should reject an account it does not own");

    assert!(
        error.contains("the group account is not the owner"),
        "expected an ownership failure, got: {error}"
    );
}
