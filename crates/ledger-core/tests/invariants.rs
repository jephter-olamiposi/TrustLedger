//! Property tests for balance conservation, structural invariants, and replay.

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::error::LedgerError;
use ledger_core::id::{AccountId, TransferId};
use ledger_core::journal::LedgerEvent;
use ledger_core::ledger::Ledger;
use ledger_core::transfer::Transfer;

const USER_COUNT: usize = 5;
const SEED_PER_USER: u128 = 100_000_000_000;

/// Independent model of the ledger balances, indexed like `shadow[0]` = pool,
/// `shadow[i + 1]` = user `i`. The property test asserts the ledger equals this
/// model after every action, so drift is caught the moment it is introduced.
#[derive(Clone, Copy, Debug, Default)]
struct Shadow {
    debits_posted: u128,
    credits_posted: u128,
    debits_pending: u128,
    credits_pending: u128,
}

impl Shadow {
    fn available_liability(&self) -> u128 {
        self.credits_posted
            .saturating_sub(self.debits_posted + self.debits_pending)
    }

    fn available_asset(&self) -> u128 {
        self.debits_posted
            .saturating_sub(self.credits_posted + self.credits_pending)
    }
}

#[derive(Clone, Debug)]
enum Action {
    Transfer {
        from: usize,
        to: usize,
        amount: u64,
    },
    PendingHold {
        from: usize,
        to: usize,
        amount: u64,
    },
    PostPending {
        pending_idx: usize,
        fraction_pct: u8,
    },
    VoidPending {
        pending_idx: usize,
    },
    CloseAccount {
        account_idx: usize,
    },
}

prop_compose! {
    fn arb_action(account_count: usize)(
        kind in 0..20u32,
        from in 0..account_count,
        to in 0..account_count,
        amount in 1u64..100_000_000u64,
        pending_idx in 0..100usize,
        fraction_pct in 1u8..=100u8,
    ) -> Action {
        match kind {
            0..=7 => Action::Transfer { from, to, amount },
            8..=12 => Action::PendingHold { from, to, amount },
            13..=15 => Action::PostPending { pending_idx, fraction_pct },
            16..=17 => Action::VoidPending { pending_idx },
            _ => Action::CloseAccount { account_idx: from },
        }
    }
}

fn seeded_ledger() -> (Ledger, AccountId, Vec<AccountId>, Vec<Shadow>) {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            0,
        )
        .expect("seed pool");

    let mut user_ids = Vec::with_capacity(USER_COUNT);
    let mut shadow = vec![Shadow::default(); USER_COUNT + 1];

    for i in 0..USER_COUNT {
        let id = AccountId::new(100 + i as u128);
        ledger
            .create_account(
                id,
                AccountType::Liability,
                AccountFlags::customer(),
                Scale::usdc(),
                (i + 1) as u64,
            )
            .expect("seed user");
        user_ids.push(id);

        let seed_tx = Transfer::new_immediate(
            TransferId::new(i as u128 + 1),
            pool,
            id,
            Amount::new(SEED_PER_USER),
            (i + 2) as u64,
        )
        .expect("seed transfer");
        ledger.create_transfer(seed_tx).expect("apply seed");

        shadow[0].debits_posted += SEED_PER_USER;
        shadow[i + 1].credits_posted += SEED_PER_USER;
    }

    (ledger, pool, user_ids, shadow)
}

fn assert_shadow_consistent(
    ledger: &Ledger,
    pool: AccountId,
    user_ids: &[AccountId],
    shadow: &[Shadow],
) -> Result<(), TestCaseError> {
    for (i, id) in user_ids.iter().enumerate() {
        let account = ledger
            .get_account(*id)
            .map_err(|_| TestCaseError::fail("user account missing"))?;
        let expected = shadow[i + 1];
        prop_assert_eq!(
            account.balance.debits_posted.as_u128(),
            expected.debits_posted
        );
        prop_assert_eq!(
            account.balance.credits_posted.as_u128(),
            expected.credits_posted
        );
        prop_assert_eq!(
            account.balance.debits_pending.as_u128(),
            expected.debits_pending
        );
        prop_assert_eq!(
            account.balance.credits_pending.as_u128(),
            expected.credits_pending
        );
        let available = account
            .balance
            .available_liability()
            .map_err(|_| TestCaseError::fail("availability overflow"))?;
        prop_assert_eq!(available.as_u128(), expected.available_liability());
    }

    let account = ledger
        .get_account(pool)
        .map_err(|_| TestCaseError::fail("pool account missing"))?;
    let expected = shadow[0];
    prop_assert_eq!(
        account.balance.debits_posted.as_u128(),
        expected.debits_posted
    );
    prop_assert_eq!(
        account.balance.credits_posted.as_u128(),
        expected.credits_posted
    );
    prop_assert_eq!(
        account.balance.debits_pending.as_u128(),
        expected.debits_pending
    );
    prop_assert_eq!(
        account.balance.credits_pending.as_u128(),
        expected.credits_pending
    );
    let available = account
        .balance
        .available_asset()
        .map_err(|_| TestCaseError::fail("availability overflow"))?;
    prop_assert_eq!(available.as_u128(), expected.available_asset());

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn prop_state_matches_shadow_model_and_replays(
        actions in proptest::collection::vec(arb_action(USER_COUNT), 1..120)
    ) {
        let (mut ledger, pool, user_ids, mut shadow) = seeded_ledger();
        let mut next_tx_id = 1000u128;
        // Clock starts far above the seeding timestamps and advances strictly per action.
        let mut clock = 1_000_000u64;
        let mut active_pending: Vec<(TransferId, usize, usize, Amount)> = Vec::new();

        for action in actions {
            clock += 1;

            match action {
                Action::Transfer { from, to, amount } => {
                    let from_id = user_ids[from % USER_COUNT];
                    let to_id = user_ids[to % USER_COUNT];
                    if from_id == to_id {
                        continue;
                    }
                    let tx = Transfer::new_immediate(
                        TransferId::new(next_tx_id),
                        from_id,
                        to_id,
                        Amount::new(amount as u128),
                        clock,
                    )
                    .expect("construct transfer");
                    next_tx_id += 1;
                    if ledger.create_transfer(tx).is_ok() {
                        shadow[from % USER_COUNT + 1].debits_posted += amount as u128;
                        shadow[to % USER_COUNT + 1].credits_posted += amount as u128;
                    }
                }
                Action::PendingHold { from, to, amount } => {
                    let from_id = user_ids[from % USER_COUNT];
                    let to_id = user_ids[to % USER_COUNT];
                    if from_id == to_id {
                        continue;
                    }
                    let tx_id = TransferId::new(next_tx_id);
                    next_tx_id += 1;
                    let tx = Transfer::new_pending(
                        tx_id,
                        from_id,
                        to_id,
                        Amount::new(amount as u128),
                        clock,
                    )
                    .expect("construct pending");
                    if ledger.create_pending(tx).is_ok() {
                        let from_shadow = from % USER_COUNT + 1;
                        let to_shadow = to % USER_COUNT + 1;
                        shadow[from_shadow].debits_pending += amount as u128;
                        shadow[to_shadow].credits_pending += amount as u128;
                        active_pending.push((tx_id, from_shadow, to_shadow, Amount::new(amount as u128)));
                    }
                }
                Action::PostPending { pending_idx, fraction_pct } => {
                    if active_pending.is_empty() {
                        continue;
                    }
                    let idx = pending_idx % active_pending.len();
                    let (pending_id, from_shadow, to_shadow, pending_amount) =
                        active_pending.remove(idx);
                    let post_amount = ((pending_amount.as_u128() * fraction_pct as u128) / 100)
                        .max(1)
                        .min(pending_amount.as_u128());
                    let post_tx_id = TransferId::new(next_tx_id);
                    next_tx_id += 1;
                    if ledger
                        .post_pending(
                            pending_id,
                            post_tx_id,
                            Amount::new(post_amount),
                            clock,
                        )
                        .is_ok()
                    {
                        shadow[from_shadow].debits_pending -= pending_amount.as_u128();
                        shadow[from_shadow].debits_posted += post_amount;
                        shadow[to_shadow].credits_pending -= pending_amount.as_u128();
                        shadow[to_shadow].credits_posted += post_amount;
                    }
                }
                Action::VoidPending { pending_idx } => {
                    if active_pending.is_empty() {
                        continue;
                    }
                    let idx = pending_idx % active_pending.len();
                    let (pending_id, from_shadow, to_shadow, pending_amount) =
                        active_pending.remove(idx);
                    if ledger.void_pending(pending_id, clock).is_ok() {
                        shadow[from_shadow].debits_pending -= pending_amount.as_u128();
                        shadow[to_shadow].credits_pending -= pending_amount.as_u128();
                    }
                }
                Action::CloseAccount { account_idx } => {
                    let id = user_ids[account_idx % USER_COUNT];
                    let _ = ledger.close_account(id, clock);
                }
            }

            prop_assert!(ledger.verify_invariants().is_ok());
            assert_shadow_consistent(&ledger, pool, &user_ids, &shadow)?;
        }

        let journal = ledger.journal();
        let replayed = Ledger::replay(Scale::usdc(), journal).expect("replay must succeed");
        prop_assert_eq!(&ledger, &replayed);
        prop_assert!(replayed.verify_invariants().is_ok());
        assert_shadow_consistent(&replayed, pool, &user_ids, &shadow)?;
    }

    #[test]
    fn prop_corrupted_void_amount_fails_replay(
        actions in proptest::collection::vec(arb_action(USER_COUNT), 10..80)
    ) {
        let (mut ledger, _pool, user_ids, _shadow) = seeded_ledger();
        let mut next_tx_id = 1000u128;
        let mut clock = 1_000_000u64;
        let mut active_pending: Vec<(TransferId, usize, usize, Amount)> = Vec::new();

        for action in actions {
            clock += 1;
            match action {
                Action::Transfer { from, to, amount } => {
                    let from_id = user_ids[from % USER_COUNT];
                    let to_id = user_ids[to % USER_COUNT];
                    if from_id == to_id {
                        continue;
                    }
                    let tx = Transfer::new_immediate(
                        TransferId::new(next_tx_id),
                        from_id,
                        to_id,
                        Amount::new(amount as u128),
                        clock,
                    )
                    .expect("construct transfer");
                    next_tx_id += 1;
                    let _ = ledger.create_transfer(tx);
                }
                Action::PendingHold { from, to, amount } => {
                    let from_id = user_ids[from % USER_COUNT];
                    let to_id = user_ids[to % USER_COUNT];
                    if from_id == to_id {
                        continue;
                    }
                    let tx_id = TransferId::new(next_tx_id);
                    next_tx_id += 1;
                    let tx = Transfer::new_pending(
                        tx_id,
                        from_id,
                        to_id,
                        Amount::new(amount as u128),
                        clock,
                    )
                    .expect("construct pending");
                    if ledger.create_pending(tx).is_ok() {
                        active_pending.push((
                            tx_id,
                            from % USER_COUNT + 1,
                            to % USER_COUNT + 1,
                            Amount::new(amount as u128),
                        ));
                    }
                }
                Action::PostPending { pending_idx, fraction_pct } => {
                    if active_pending.is_empty() {
                        continue;
                    }
                    let idx = pending_idx % active_pending.len();
                    let (pending_id, _fh, _th, pending_amount) = active_pending.remove(idx);
                    let post_amount = ((pending_amount.as_u128() * fraction_pct as u128) / 100)
                        .max(1)
                        .min(pending_amount.as_u128());
                    let post_tx_id = TransferId::new(next_tx_id);
                    next_tx_id += 1;
                    let _ = ledger.post_pending(pending_id, post_tx_id, Amount::new(post_amount), clock);
                }
                Action::VoidPending { pending_idx } => {
                    if active_pending.is_empty() {
                        continue;
                    }
                    let idx = pending_idx % active_pending.len();
                    let (pending_id, _fh, _th, _amount) = active_pending.remove(idx);
                    let _ = ledger.void_pending(pending_id, clock);
                }
                Action::CloseAccount { account_idx } => {
                    let id = user_ids[account_idx % USER_COUNT];
                    let _ = ledger.close_account(id, clock);
                }
            }
        }

        let events = ledger.journal();
        let void_index = events.iter().position(|event| {
            matches!(event, LedgerEvent::TransferPendingVoided { .. })
        });
        let Some(void_index) = void_index else {
            // No void event was generated in this run; nothing to corrupt.
            return Ok(());
        };

        let mut corrupt = events.to_vec();
        let pending_id = match &corrupt[void_index] {
            LedgerEvent::TransferPendingVoided { pending_id, .. } => *pending_id,
            _ => unreachable!("filtered above"),
        };
        corrupt[void_index] = LedgerEvent::TransferPendingVoided {
            pending_id,
            amount: Amount::new(u64::MAX as u128),
            timestamp: 0,
        };

        let err = Ledger::replay(Scale::usdc(), &corrupt)
            .expect_err("corrupted void amount must break replay");
        let is_mismatch = matches!(err, LedgerError::JournalVoidedAmountMismatch { .. });
        prop_assert!(is_mismatch, "expected JournalVoidedAmountMismatch, got {err:?}");
    }

    #[test]
    fn prop_amount_checked_arithmetic(a in 0u64..1_000_000_000, b in 0u64..1_000_000_000) {
        let amt_a = Amount::new(a as u128);
        let amt_b = Amount::new(b as u128);

        let sum_1 = amt_a.checked_add(amt_b).unwrap();
        let sum_2 = amt_b.checked_add(amt_a).unwrap();
        prop_assert_eq!(sum_1, sum_2);

        let restored = sum_1.checked_sub(amt_b).unwrap();
        prop_assert_eq!(restored, amt_a);
    }
}
