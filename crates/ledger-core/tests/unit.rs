//! Unit tests for ledger-core.

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::error::LedgerError;
use ledger_core::id::{AccountId, TransferId};
use ledger_core::journal::LedgerEvent;
use ledger_core::ledger::Ledger;
use ledger_core::transfer::{Transfer, TransferState};

#[test]
fn test_account_creation_and_scale_validation() {
    let mut ledger = Ledger::new(Scale::usdc());
    let alice = AccountId::new(1);

    assert!(ledger
        .create_account(
            alice,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            100
        )
        .is_ok());

    let dup_err = ledger
        .create_account(
            alice,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            101,
        )
        .unwrap_err();
    assert_eq!(dup_err, LedgerError::AccountAlreadyExists(alice));

    let bob = AccountId::new(2);
    let scale_err = ledger
        .create_account(
            bob,
            AccountType::Asset,
            AccountFlags::bank_asset(),
            Scale::usd(),
            102,
        )
        .unwrap_err();
    assert_eq!(
        scale_err,
        LedgerError::ScaleMismatch {
            expected: Scale::usdc(),
            actual: Scale::usd()
        }
    );
}

#[test]
fn test_immediate_transfer_success_and_invariants() {
    let mut ledger = Ledger::new(Scale::usdc());
    let vault = AccountId::new(1);
    let merchant = AccountId::new(2);

    ledger
        .create_account(
            vault,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .expect("create vault");
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .expect("create merchant");

    let transfer = Transfer::new_immediate(
        TransferId::new(100),
        vault,
        merchant,
        Amount::new(5_000_000),
        10,
    )
    .expect("create transfer");

    assert!(ledger.create_transfer(transfer).is_ok());

    let vault_acc = ledger.get_account(vault).expect("vault account");
    let merchant_acc = ledger.get_account(merchant).expect("merchant account");

    assert_eq!(vault_acc.balance.debits_posted, Amount::new(5_000_000));
    assert_eq!(merchant_acc.balance.credits_posted, Amount::new(5_000_000));

    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_two_phase_transfer_full_capture() {
    let mut ledger = Ledger::new(Scale::usdc());
    let customer = AccountId::new(10);
    let merchant = AccountId::new(20);
    let settlement_pool = AccountId::new(30);

    ledger
        .create_account(
            settlement_pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .expect("create pool");
    ledger
        .create_account(
            customer,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .expect("create customer");
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .expect("create merchant");

    let deposit = Transfer::new_immediate(
        TransferId::new(1),
        settlement_pool,
        customer,
        Amount::new(100_000_000),
        2,
    )
    .expect("deposit");
    ledger.create_transfer(deposit).expect("apply deposit");

    assert_eq!(
        ledger
            .get_account(customer)
            .unwrap()
            .balance
            .available_liability()
            .unwrap(),
        Amount::new(100_000_000)
    );

    let hold = Transfer::new_pending(
        TransferId::new(2),
        customer,
        merchant,
        Amount::new(40_000_000),
        3,
    )
    .expect("hold");
    ledger.create_pending(hold).expect("apply pending");

    assert_eq!(
        ledger
            .get_account(customer)
            .unwrap()
            .balance
            .available_liability()
            .unwrap(),
        Amount::new(60_000_000)
    );

    ledger
        .post_pending(
            TransferId::new(2),
            TransferId::new(3),
            Amount::new(40_000_000),
            4,
        )
        .expect("post pending");

    let customer_acc = ledger.get_account(customer).unwrap();
    let merchant_acc = ledger.get_account(merchant).unwrap();

    assert_eq!(customer_acc.balance.debits_posted, Amount::new(40_000_000));
    assert_eq!(customer_acc.balance.debits_pending, Amount::ZERO);
    assert_eq!(
        customer_acc.balance.available_liability().unwrap(),
        Amount::new(60_000_000)
    );

    assert_eq!(merchant_acc.balance.credits_posted, Amount::new(40_000_000));
    assert_eq!(merchant_acc.balance.credits_pending, Amount::ZERO);

    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_two_phase_transfer_partial_capture() {
    let mut ledger = Ledger::new(Scale::usdc());
    let customer = AccountId::new(10);
    let merchant = AccountId::new(20);
    let settlement_pool = AccountId::new(30);

    ledger
        .create_account(
            settlement_pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            customer,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(1),
                settlement_pool,
                customer,
                Amount::new(50_000_000),
                2,
            )
            .unwrap(),
        )
        .unwrap();

    ledger
        .create_pending(
            Transfer::new_pending(
                TransferId::new(2),
                customer,
                merchant,
                Amount::new(30_000_000),
                3,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(
        ledger
            .get_account(customer)
            .unwrap()
            .balance
            .available_liability()
            .unwrap(),
        Amount::new(20_000_000)
    );

    ledger
        .post_pending(
            TransferId::new(2),
            TransferId::new(3),
            Amount::new(10_000_000),
            4,
        )
        .expect("partial capture");

    let customer_acc = ledger.get_account(customer).unwrap();
    assert_eq!(
        customer_acc.balance.available_liability().unwrap(),
        Amount::new(40_000_000)
    );
    assert_eq!(customer_acc.balance.debits_posted, Amount::new(10_000_000));
    assert_eq!(customer_acc.balance.debits_pending, Amount::ZERO);

    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_two_phase_transfer_void() {
    let mut ledger = Ledger::new(Scale::usdc());
    let customer = AccountId::new(10);
    let merchant = AccountId::new(20);
    let pool = AccountId::new(30);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            customer,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(1),
                pool,
                customer,
                Amount::new(50_000_000),
                2,
            )
            .unwrap(),
        )
        .unwrap();

    ledger
        .create_pending(
            Transfer::new_pending(
                TransferId::new(2),
                customer,
                merchant,
                Amount::new(25_000_000),
                3,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(
        ledger
            .get_account(customer)
            .unwrap()
            .balance
            .available_liability()
            .unwrap(),
        Amount::new(25_000_000)
    );

    ledger
        .void_pending(TransferId::new(2), 4)
        .expect("void pending");

    let customer_acc = ledger.get_account(customer).unwrap();
    assert_eq!(
        customer_acc.balance.available_liability().unwrap(),
        Amount::new(50_000_000)
    );
    assert_eq!(customer_acc.balance.debits_pending, Amount::ZERO);

    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_overdraft_protection() {
    let mut ledger = Ledger::new(Scale::usdc());
    let customer = AccountId::new(1);
    let merchant = AccountId::new(2);

    ledger
        .create_account(
            customer,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    let err = ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(100),
                customer,
                merchant,
                Amount::new(10_000_000),
                2,
            )
            .unwrap(),
        )
        .unwrap_err();

    assert!(matches!(err, LedgerError::InsufficientFunds { .. }));
}

#[test]
fn test_deterministic_replay_convergence() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user_a = AccountId::new(2);
    let user_b = AccountId::new(3);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_a,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_b,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(10),
                pool,
                user_a,
                Amount::new(200_000_000),
                2,
            )
            .unwrap(),
        )
        .unwrap();

    ledger
        .create_pending(
            Transfer::new_pending(
                TransferId::new(11),
                user_a,
                user_b,
                Amount::new(50_000_000),
                3,
            )
            .unwrap(),
        )
        .unwrap();

    ledger
        .post_pending(
            TransferId::new(11),
            TransferId::new(12),
            Amount::new(30_000_000),
            4,
        )
        .unwrap();

    let journal = ledger.journal();
    let replayed = Ledger::replay(Scale::usdc(), journal).expect("replay should succeed");

    assert_eq!(ledger, replayed);
    assert!(replayed.verify_invariants().is_ok());
}

#[test]
fn test_net_settled_bounds_and_direction() {
    let mut balance = ledger_core::account::Balance::new();
    balance.debits_posted = Amount::new(300);
    balance.credits_posted = Amount::new(100);

    assert_eq!(
        balance.net_settled(AccountType::Asset).unwrap(),
        200,
        "asset net balance is debits minus credits"
    );
    assert_eq!(
        balance.net_settled(AccountType::Liability).unwrap(),
        -200,
        "liability net balance is credits minus debits"
    );

    let mut overflow = ledger_core::account::Balance::new();
    overflow.debits_posted = Amount::new(i128::MAX as u128 + 1);
    assert_eq!(
        overflow.net_settled(AccountType::Asset).unwrap_err(),
        LedgerError::SignedBalanceOverflow,
        "components above i128::MAX must error instead of silently wrapping"
    );
}

#[test]
fn test_scale_validation_and_rejection() {
    assert!(Scale::new(18).is_ok());
    assert_eq!(
        Scale::new(19).unwrap_err(),
        LedgerError::InvalidScale(19),
        "scale above 18 must be rejected"
    );
    assert_eq!(Scale::usd().as_u8(), 2);
    assert_eq!(Scale::usdc().as_u8(), 6);
    assert_eq!(Scale::btc().as_u8(), 8);
    assert_eq!(Scale::eth().as_u8(), 18);
}

#[test]
fn test_insufficient_funds_reports_accurate_available_balance() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let customer = AccountId::new(2);
    let merchant = AccountId::new(3);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            customer,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            merchant,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(10),
                pool,
                customer,
                Amount::new(100_000_000),
                2,
            )
            .unwrap(),
        )
        .unwrap();
    ledger
        .create_pending(
            Transfer::new_pending(
                TransferId::new(11),
                customer,
                merchant,
                Amount::new(40_000_000),
                3,
            )
            .unwrap(),
        )
        .unwrap();
    let err = ledger
        .create_transfer(
            Transfer::new_immediate(
                TransferId::new(12),
                customer,
                merchant,
                Amount::new(70_000_000),
                4,
            )
            .unwrap(),
        )
        .unwrap_err();

    assert_eq!(
        err,
        LedgerError::InsufficientFunds {
            account_id: customer,
            requested: Amount::new(70_000_000),
            available: Amount::new(60_000_000),
        }
    );
}

#[test]
fn test_available_balance_overflow_propagation() {
    let mut balance = ledger_core::account::Balance::new();
    balance.debits_posted = Amount::new(u128::MAX);
    balance.debits_pending = Amount::new(1);

    assert_eq!(
        balance.available_liability().unwrap_err(),
        LedgerError::ArithmeticOverflow,
        "overflow in debit summation must propagate as ArithmeticOverflow"
    );
}

#[test]
fn test_apply_batch_success() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user_a = AccountId::new(2);
    let user_b = AccountId::new(3);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_a,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_b,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    let batch = vec![
        Transfer::new_immediate(TransferId::new(1), pool, user_a, Amount::new(100), 2).unwrap(),
        Transfer::new_immediate(TransferId::new(2), pool, user_b, Amount::new(200), 3).unwrap(),
    ];

    assert!(ledger.apply_batch(&batch).is_ok());
    assert_eq!(
        ledger.get_account(user_a).unwrap().balance.credits_posted,
        Amount::new(100)
    );
    assert_eq!(
        ledger.get_account(user_b).unwrap().balance.credits_posted,
        Amount::new(200)
    );
    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_apply_batch_atomic_rollback() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user_a = AccountId::new(2);
    let user_b = AccountId::new(3);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_a,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user_b,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(1), pool, user_a, Amount::new(100), 2).unwrap(),
        )
        .unwrap();
    let batch = vec![
        Transfer::new_immediate(TransferId::new(2), user_a, user_b, Amount::new(40), 3).unwrap(),
        Transfer::new_immediate(TransferId::new(3), user_a, user_b, Amount::new(80), 4).unwrap(),
    ];

    assert!(ledger.apply_batch(&batch).is_err());
    assert_eq!(
        ledger.get_account(user_a).unwrap().balance.debits_posted,
        Amount::ZERO,
        "first transfer in failed batch must be rolled back"
    );
    assert_eq!(
        ledger.get_account(user_b).unwrap().balance.credits_posted,
        Amount::ZERO,
        "recipient in failed batch must have zero credits"
    );
    assert!(ledger.verify_invariants().is_ok());
}

#[test]
fn test_invalid_transfer_state_typed_error() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            1,
        )
        .unwrap();

    let pending =
        Transfer::new_pending(TransferId::new(1), pool, user, Amount::new(50), 2).unwrap();
    let err = ledger.create_transfer(pending).unwrap_err();
    assert_eq!(
        err,
        LedgerError::InvalidTransferState {
            id: TransferId::new(1),
            expected: ledger_core::transfer::TransferState::Posted,
            actual: ledger_core::transfer::TransferState::Pending,
        }
    );
}

#[test]
fn test_timestamp_monotonicity_rejects_out_of_order_events() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            10,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            20,
        )
        .unwrap();

    ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(1), pool, user, Amount::new(100), 20).unwrap(),
        )
        .unwrap();

    let err = ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(2), pool, user, Amount::new(100), 15).unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::TimestampBehindPrior {
            timestamp: 15,
            last: 20,
        }
    );

    ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(3), pool, user, Amount::new(100), 25).unwrap(),
        )
        .unwrap();
}

#[test]
fn test_timestamp_monotonicity_applies_to_post_and_void() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();
    ledger
        .create_pending(
            Transfer::new_pending(TransferId::new(10), pool, user, Amount::new(50), 3).unwrap(),
        )
        .unwrap();

    let err = ledger
        .post_pending(TransferId::new(10), TransferId::new(11), Amount::new(50), 2)
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::TimestampBehindPrior {
            timestamp: 2,
            last: 3,
        }
    );

    ledger
        .post_pending(TransferId::new(10), TransferId::new(11), Amount::new(50), 4)
        .unwrap();

    ledger
        .create_pending(
            Transfer::new_pending(TransferId::new(12), pool, user, Amount::new(30), 5).unwrap(),
        )
        .unwrap();
    let err = ledger.void_pending(TransferId::new(12), 4).unwrap_err();
    assert_eq!(
        err,
        LedgerError::TimestampBehindPrior {
            timestamp: 4,
            last: 5,
        }
    );
}

#[test]
fn test_close_account_rejects_new_transfers() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();

    ledger.close_account(user, 3).unwrap();
    assert!(ledger.get_account(user).unwrap().flags.is_closed);

    let err = ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(1), pool, user, Amount::new(50), 4).unwrap(),
        )
        .unwrap_err();
    assert_eq!(err, LedgerError::AccountClosed(user));

    let err = ledger
        .create_transfer(
            Transfer::new_immediate(TransferId::new(2), user, pool, Amount::new(50), 5).unwrap(),
        )
        .unwrap_err();
    assert_eq!(err, LedgerError::AccountClosed(user));

    let err = ledger.close_account(user, 6).unwrap_err();
    assert_eq!(err, LedgerError::AccountClosed(user));

    let ghost = AccountId::new(7);
    let err = ledger.close_account(ghost, 7).unwrap_err();
    assert_eq!(err, LedgerError::AccountNotFound(ghost));

    let replayed = Ledger::replay(Scale::usdc(), ledger.journal()).expect("replay closed ledger");
    assert_eq!(ledger, replayed);
    assert!(replayed.verify_invariants().is_ok());
}

#[test]
fn test_pending_lifecycle_rejection_matrix() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();

    let pending_id = TransferId::new(100);
    let post_id = TransferId::new(101);
    ledger
        .create_pending(Transfer::new_pending(pending_id, pool, user, Amount::new(50), 3).unwrap())
        .unwrap();

    ledger.void_pending(pending_id, 4).unwrap();
    let err = ledger
        .post_pending(pending_id, post_id, Amount::new(50), 5)
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::InvalidTransferState {
            id: pending_id,
            expected: TransferState::Pending,
            actual: TransferState::Voided,
        }
    );

    let err = ledger.void_pending(pending_id, 6).unwrap_err();
    assert_eq!(
        err,
        LedgerError::InvalidTransferState {
            id: pending_id,
            expected: TransferState::Pending,
            actual: TransferState::Voided,
        }
    );

    let mut post_ledger = Ledger::new(Scale::usdc());
    post_ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    post_ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();
    post_ledger
        .create_pending(Transfer::new_pending(pending_id, pool, user, Amount::new(50), 3).unwrap())
        .unwrap();
    post_ledger
        .post_pending(pending_id, post_id, Amount::new(50), 4)
        .unwrap();
    let err = post_ledger.void_pending(pending_id, 5).unwrap_err();
    assert_eq!(
        err,
        LedgerError::InvalidTransferState {
            id: pending_id,
            expected: TransferState::Pending,
            actual: TransferState::Posted,
        }
    );

    let err = post_ledger
        .post_pending(pending_id, TransferId::new(102), Amount::new(50), 6)
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::InvalidTransferState {
            id: pending_id,
            expected: TransferState::Pending,
            actual: TransferState::Posted,
        }
    );
}

#[test]
fn test_transfer_id_uniqueness_rejected() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();

    let transfer =
        Transfer::new_immediate(TransferId::new(5), pool, user, Amount::new(50), 3).unwrap();
    ledger.create_transfer(transfer).unwrap();

    let duplicate =
        Transfer::new_immediate(TransferId::new(5), user, pool, Amount::new(10), 4).unwrap();
    let err = ledger.create_transfer(duplicate).unwrap_err();
    assert_eq!(err, LedgerError::TransferAlreadyExists(TransferId::new(5)));

    let pending =
        Transfer::new_pending(TransferId::new(6), pool, user, Amount::new(50), 5).unwrap();
    ledger.create_pending(pending).unwrap();
    let dup_pending =
        Transfer::new_pending(TransferId::new(6), user, pool, Amount::new(10), 6).unwrap();
    let err = ledger.create_pending(dup_pending).unwrap_err();
    assert_eq!(err, LedgerError::TransferAlreadyExists(TransferId::new(6)));
}

#[test]
fn test_constructor_rejections_and_partial_exceeds() {
    let same = AccountId::new(1);
    let err =
        Transfer::new_immediate(TransferId::new(1), same, same, Amount::new(50), 1).unwrap_err();
    assert_eq!(err, LedgerError::DebitCreditAccountSame(same));

    let err = Transfer::new_immediate(TransferId::new(2), same, AccountId::new(2), Amount::ZERO, 1)
        .unwrap_err();
    assert_eq!(err, LedgerError::ZeroAmountTransfer(TransferId::new(2)));

    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);
    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();

    ledger
        .create_pending(
            Transfer::new_pending(TransferId::new(10), pool, user, Amount::new(50), 3).unwrap(),
        )
        .unwrap();

    let err = ledger
        .post_pending(TransferId::new(10), TransferId::new(11), Amount::new(51), 4)
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::PartialAmountExceedsPending {
            transfer_id: TransferId::new(10),
            requested: Amount::new(51),
            pending: Amount::new(50),
        }
    );

    let err = ledger
        .post_pending(TransferId::new(10), TransferId::new(11), Amount::ZERO, 4)
        .unwrap_err();
    assert_eq!(err, LedgerError::ZeroAmountTransfer(TransferId::new(11)));
}

#[test]
fn test_replay_rejects_corrupted_void_amount() {
    let mut ledger = Ledger::new(Scale::usdc());
    let pool = AccountId::new(1);
    let user = AccountId::new(2);

    ledger
        .create_account(
            pool,
            AccountType::Asset,
            AccountFlags::unrestricted(),
            Scale::usdc(),
            1,
        )
        .unwrap();
    ledger
        .create_account(
            user,
            AccountType::Liability,
            AccountFlags::customer(),
            Scale::usdc(),
            2,
        )
        .unwrap();
    ledger
        .create_pending(
            Transfer::new_pending(TransferId::new(10), pool, user, Amount::new(50), 3).unwrap(),
        )
        .unwrap();
    ledger.void_pending(TransferId::new(10), 4).unwrap();

    let mut events = ledger.journal().to_vec();
    let last = events.last_mut().expect("journal ends with the void event");
    *last = LedgerEvent::TransferPendingVoided {
        pending_id: TransferId::new(10),
        amount: Amount::new(999),
        timestamp: 4,
    };

    let err = Ledger::replay(Scale::usdc(), &events).unwrap_err();
    assert_eq!(
        err,
        LedgerError::JournalVoidedAmountMismatch {
            pending_id: TransferId::new(10),
            pending: Amount::new(50),
            amount: Amount::new(999),
        }
    );
}
