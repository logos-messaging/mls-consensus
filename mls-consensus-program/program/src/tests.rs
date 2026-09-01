//! Host-side unit tests for the group state transitions.
//!
//! These exercise the same functions the guest calls, without the zkVM: `cargo test` runs them in
//! milliseconds. The compiled ELF is covered separately by `methods/tests/guest.rs`.

use lee_core::{
    account::{Account, AccountId, AccountWithMetadata, Data, Nonce},
    program::{AccountPostState, Claim, ProgramId},
};
use mls_consensus_core::{ConvoState, EpochFingerprint, Instruction};

use super::{commit, execute, initialize};

const SELF_PROGRAM_ID: ProgramId = [7; 8];
const OTHER_PROGRAM_ID: ProgramId = [9; 8];
const ACCOUNT_ID: AccountId = AccountId::new([1; 32]);

/// Deliberately non-zero, so a test asserting they survive a write would notice a program that
/// reset them.
const NONCE: u128 = 4_242;
const BALANCE: u128 = 1_000;

fn uninitialized() -> AccountWithMetadata {
    AccountWithMetadata {
        account: Account::default(),
        is_authorized: true,
        account_id: ACCOUNT_ID,
    }
}

fn holding(state: &ConvoState, owner: ProgramId) -> AccountWithMetadata {
    with_data(Data::try_from(state).expect("state should encode"), owner)
}

fn with_data(data: Data, owner: ProgramId) -> AccountWithMetadata {
    AccountWithMetadata {
        account: Account {
            program_owner: AccountId::from(owner),
            balance: BALANCE,
            data,
            nonce: Nonce::from(NONCE),
        },
        is_authorized: true,
        account_id: ACCOUNT_ID,
    }
}

fn state_in(post_state: &AccountPostState) -> ConvoState {
    ConvoState::try_from(&post_state.account().data).expect("post state should hold a ConvoState")
}

/// A distinguishable fingerprint. The bytes are arbitrary — the program never reads them.
fn fingerprint(byte: u8) -> EpochFingerprint {
    EpochFingerprint::from([byte; 32])
}

#[test]
fn initialize_claims_the_account_and_starts_an_empty_group() {
    let post_state = initialize(uninitialized());

    assert_eq!(post_state.required_claim(), Some(Claim::Authorized));

    let state = state_in(&post_state);
    assert_eq!(state.version, 0);
    assert!(state.epoch_history.is_empty());
}

#[test]
#[should_panic(expected = "initialize: the group account must authorize")]
fn initialize_requires_authorization() {
    let _ = initialize(AccountWithMetadata {
        is_authorized: false,
        ..uninitialized()
    });
}

#[test]
#[should_panic(expected = "initialize: the group account must be uninitialized")]
fn initialize_refuses_an_account_that_already_holds_a_group() {
    let _ = initialize(holding(&ConvoState::new(), SELF_PROGRAM_ID));
}

#[test]
fn commit_appends_to_the_history_and_bumps_the_version() {
    let post_state = commit(
        holding(&ConvoState::new(), SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        0,
        fingerprint(1),
    );

    let state = state_in(&post_state);
    assert_eq!(state.version, 1);
    assert_eq!(state.epoch_history, vec![fingerprint(1)]);
}

#[test]
fn commits_accumulate_oldest_first() {
    let first = commit(
        holding(&ConvoState::new(), SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        0,
        fingerprint(1),
    );
    let second = commit(
        holding(&state_in(&first), SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        1,
        fingerprint(2),
    );

    let state = state_in(&second);
    assert_eq!(state.version, 2);
    assert_eq!(state.epoch_history, vec![fingerprint(1), fingerprint(2)]);
}

#[test]
fn commit_rewrites_nothing_but_the_data_field() {
    let pre = holding(&ConvoState::new(), SELF_PROGRAM_ID);
    let before = pre.account.clone();

    let post_state = commit(pre, SELF_PROGRAM_ID, 0, fingerprint(1));
    let after = post_state.account();

    assert_eq!(after.nonce, before.nonce);
    assert_eq!(after.balance, before.balance);
    assert_eq!(after.program_owner, before.program_owner);
    assert_ne!(after.data, before.data);
}

#[test]
#[should_panic(expected = "commit: version conflict")]
fn commit_against_a_stale_version_is_rejected() {
    let mut state = ConvoState::new();
    state.version = 3;

    let _ = commit(
        holding(&state, SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        1,
        fingerprint(1),
    );
}

#[test]
#[should_panic(expected = "commit: the group account must authorize")]
fn commit_requires_authorization() {
    let _ = commit(
        AccountWithMetadata {
            is_authorized: false,
            ..holding(&ConvoState::new(), SELF_PROGRAM_ID)
        },
        SELF_PROGRAM_ID,
        0,
        fingerprint(1),
    );
}

#[test]
#[should_panic(expected = "commit: the group account is not the owner")]
fn commit_refuses_an_account_owned_by_another_program() {
    let _ = commit(
        holding(&ConvoState::new(), OTHER_PROGRAM_ID),
        SELF_PROGRAM_ID,
        0,
        fingerprint(1),
    );
}

#[test]
#[should_panic(expected = "commit: account data is not a valid ConvoState")]
fn commit_refuses_data_that_is_not_a_group_state() {
    let data = Data::try_from(b"not a ConvoState".to_vec()).expect("small enough for an account");

    let _ = commit(
        with_data(data, SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        0,
        fingerprint(1),
    );
}

#[test]
#[should_panic(expected = "version counter overflowed")]
fn the_version_counter_refuses_to_wrap() {
    let state = ConvoState {
        version: u64::MAX,
        epoch_history: vec![],
    };

    let _ = commit(
        holding(&state, SELF_PROGRAM_ID),
        SELF_PROGRAM_ID,
        u64::MAX,
        fingerprint(1),
    );
}

#[test]
fn execute_dispatches_initialize() {
    let post_states = execute(
        SELF_PROGRAM_ID,
        vec![uninitialized()],
        Instruction::Initialize {},
    );

    assert_eq!(post_states.len(), 1);
    assert_eq!(state_in(&post_states[0]).version, 0);
}

#[test]
fn execute_dispatches_commit() {
    let post_states = execute(
        SELF_PROGRAM_ID,
        vec![holding(&ConvoState::new(), SELF_PROGRAM_ID)],
        Instruction::Commit {
            expected_version: 0,
            epoch_fingerprint: fingerprint(5),
        },
    );

    assert_eq!(post_states.len(), 1);
    assert_eq!(
        state_in(&post_states[0]).epoch_history,
        vec![fingerprint(5)]
    );
}

#[test]
#[should_panic(expected = "exactly one account")]
fn execute_refuses_anything_but_one_account() {
    let _ = execute(SELF_PROGRAM_ID, vec![], Instruction::Initialize {});
}
