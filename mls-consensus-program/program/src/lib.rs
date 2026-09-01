//! State transitions for the MLS consensus program.
//!
//! LEE programs are stateless: they receive the accounts they are allowed to touch, and they
//! return the proposed new contents of those accounts. Everything in this crate is therefore a
//! plain function from pre-states to post-states, with no I/O and no zkVM dependency. The guest
//! binary in `methods/guest` is a thin shim that reads the inputs, calls [`execute`], and writes
//! the result; keeping the logic here means it can be unit-tested on the host in milliseconds.
//!
//! # What the program decides
//!
//! One account holds one group's [`ConvoState`]. MLS decides what a commit *means*; this program
//! decides only which commit is next. Every mutating instruction names the version it expects to
//! replace, so two members committing against the same epoch cannot both succeed: one lands, and
//! the other is rejected and rebuilds against the head that won. Without that, the two members
//! apply different commits and the group forks into halves that can no longer decrypt each other.
//!
//! Nothing here interprets a commit. An epoch fingerprint is opaque bytes to this program, which
//! is what lets a group put its ordering under consensus without putting its contents there.
//!
//! # What the runtime enforces, and what this program must enforce
//!
//! LEE validates every proposed transition (`lee_core::program::validate_execution`) and will
//! reject the whole transaction if a program tries to change an account's `nonce`, its
//! `program_owner`, or the total balance across accounts. These functions therefore only ever
//! rewrite the `data` field, and leave every other field of the account exactly as they found it.
//!
//! On top of that the runtime only permits a `data` change when the account is either
//! *uninitialized* or *already owned by the executing program*. The checks below deliberately
//! mirror that rule rather than relying on it, so that a caller gets a precise error message
//! ("the group account is not the owner") instead of an opaque runtime rejection.
//!
//! # Failure convention
//!
//! A guest panics to reject a transaction; the panic message surfaces in the node logs as
//! `ProgramExecutionFailed`. Every assertion below is named so that the message alone explains
//! what the caller did wrong.

use lee_core::{
    account::{Account, AccountId, AccountWithMetadata, Data},
    program::{AccountPostState, Claim, ProgramId},
};
use mls_consensus_core::{ConvoState, EpochFingerprint, Instruction};

/// Applies an [`Instruction`] to the accounts it was given.
///
/// Every instruction operates on exactly one account — the group itself — so this unpacks that
/// account and dispatches. The returned vector lines up one-to-one with `pre_states`, as LEE
/// requires.
///
/// # Panics
///
/// Panics if `pre_states` does not contain exactly one account, or if the instruction's own
/// preconditions are not met.
#[must_use]
pub fn execute(
    self_program_id: ProgramId,
    pre_states: Vec<AccountWithMetadata>,
    instruction: Instruction,
) -> Vec<AccountPostState> {
    let [group_account] = <[AccountWithMetadata; 1]>::try_from(pre_states).unwrap_or_else(|got| {
        panic!(
            "mls consensus: every instruction takes exactly one account, got {}",
            got.len()
        )
    });

    let post_state = match instruction {
        Instruction::Initialize {} => initialize(group_account),
        Instruction::Commit {
            expected_version,
            epoch_fingerprint,
        } => commit(
            group_account,
            self_program_id,
            expected_version,
            epoch_fingerprint,
        ),
    };

    vec![post_state]
}

/// Creates the group and claims the account for this program.
///
/// The account must be pristine. Refusing to initialize an account that already holds something
/// is what keeps this from being a second way to spell [`commit`]: a client that believes it is
/// creating a group can never silently reset an existing one back to version 0.
///
/// # Panics
///
/// Panics if the account does not authorize the transaction, if it is not uninitialized, or if
/// the empty state somehow fails to encode.
#[must_use]
pub fn initialize(group_account: AccountWithMetadata) -> AccountPostState {
    assert!(
        group_account.is_authorized,
        "initialize: the group account must authorize the transaction"
    );
    assert!(
        group_account.account == Account::default(),
        "initialize: the group account must be uninitialized"
    );

    let convo_state = ConvoState::new();
    let post_account = write_state(group_account.account, &convo_state, "initialize");

    // The account is uninitialized, so this claims it: from here on this program is its
    // `program_owner`, and no other program can rewrite the group's state.
    AccountPostState::new_claimed_if_default(post_account, Claim::Authorized)
}

/// Advances the group by one commit, if it is still at the version the caller saw.
///
/// Appends the epoch the commit produces to the history and bumps the version. The program does
/// not interpret the fingerprint: whether the commit that produced it is well-formed MLS is a
/// question for the members, who hold the keys to answer it.
///
/// # Panics
///
/// Panics if the account does not authorize the transaction, is not owned by this program, does
/// not hold a decodable [`ConvoState`], or has moved past `expected_version`.
#[must_use]
pub fn commit(
    group_account: AccountWithMetadata,
    self_program_id: ProgramId,
    expected_version: u64,
    epoch_fingerprint: EpochFingerprint,
) -> AccountPostState {
    let current = open_for_write(&group_account, self_program_id, expected_version, "commit");

    let mut epoch_history = current.epoch_history;
    epoch_history.push(epoch_fingerprint);

    let next = ConvoState {
        version: next_version(current.version, "commit"),
        epoch_history,
    };

    AccountPostState::new(write_state(group_account.account, &next, "commit"))
}

/// Checks every precondition shared by the mutating instructions and returns the current state.
///
/// Every instruction that writes over an existing group goes through here rather than asserting
/// for itself, so there is one place to read for what a caller must hold to move a group forward.
///
/// `operation` names the caller so the panic message points at the instruction that failed.
fn open_for_write(
    group_account: &AccountWithMetadata,
    self_program_id: ProgramId,
    expected_version: u64,
    operation: &str,
) -> ConvoState {
    assert!(
        group_account.is_authorized,
        "{operation}: the group account must authorize the transaction"
    );
    assert!(
        group_account.account.program_owner == AccountId::from(self_program_id),
        "{operation}: the group account is not the owner"
    );

    let state = ConvoState::try_from(&group_account.account.data)
        .unwrap_or_else(|error| panic!("{operation}: {error}"));

    // The compare-and-swap that keeps the group from forking. The sequencer has already put
    // every transaction in a total order; this turns "two members committed against the same
    // epoch" from a silently lost update into a rejected transaction that the loser can retry
    // against the head that won.
    assert!(
        state.version == expected_version,
        "{operation}: version conflict, expected version {expected_version} but the group is at version {}",
        state.version
    );

    state
}

/// Encodes `convo_state` into `account`'s data field, leaving every other field untouched.
///
/// Validation lives here rather than at the call sites, so no path can write a state that
/// [`ConvoState::validate`] would reject.
fn write_state(mut account: Account, convo_state: &ConvoState, operation: &str) -> Account {
    convo_state
        .validate()
        .unwrap_or_else(|error| panic!("{operation}: {error}"));

    account.data =
        Data::try_from(convo_state).unwrap_or_else(|error| panic!("{operation}: {error}"));

    account
}

/// Increments a version counter, refusing to wrap.
fn next_version(current: u64, operation: &str) -> u64 {
    current
        .checked_add(1)
        .unwrap_or_else(|| panic!("{operation}: version counter overflowed"))
}

#[cfg(test)]
mod tests;
