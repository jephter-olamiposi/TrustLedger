use std::time::Duration;

use ledger_core::{AccountFlags, AccountId, AccountType, Amount, Scale, Transfer, TransferId};
use raft::{RaftCluster, RaftRequest, RaftResponse};

#[tokio::test]
async fn test_three_node_cluster_bootstrap_and_replication() {
    let scale = Scale::new(2).expect("valid scale");
    let mut cluster = RaftCluster::new_3node(scale)
        .await
        .expect("cluster should bootstrap");

    // Wait for cluster leader election
    let leader_id = cluster
        .wait_for_leader(Duration::from_secs(3))
        .await
        .expect("cluster must elect a leader");
    assert!((1..=3).contains(&leader_id));

    // 1. Create two accounts on the leader
    let acc1_id = AccountId::new(101);
    let acc2_id = AccountId::new(102);

    let res1 = cluster
        .propose(RaftRequest::CreateAccount {
            id: acc1_id,
            account_type: AccountType::Asset,
            flags: AccountFlags::bank_asset(),
            scale,
            timestamp: 1,
        })
        .await
        .expect("account 1 creation should commit");
    assert_eq!(res1, RaftResponse::AccountCreated(acc1_id));

    let res2 = cluster
        .propose(RaftRequest::CreateAccount {
            id: acc2_id,
            account_type: AccountType::Liability,
            flags: AccountFlags::customer(),
            scale,
            timestamp: 2,
        })
        .await
        .expect("account 2 creation should commit");
    assert_eq!(res2, RaftResponse::AccountCreated(acc2_id));

    // 2. Propose an immediate transfer
    let transfer_id = TransferId::new(1001);
    let amount = Amount::new(5000); // 50.00
    let transfer =
        Transfer::new_immediate(transfer_id, acc1_id, acc2_id, amount, 3).expect("valid transfer");

    let transfer_res = cluster
        .propose(RaftRequest::CreateTransfer(transfer))
        .await
        .expect("transfer proposal should commit");
    assert_eq!(transfer_res, RaftResponse::TransferCreated(transfer_id));

    // 3. Propose two-phase pending transfer hold and post
    let pending_id = TransferId::new(2001);
    let post_id = TransferId::new(2002);
    let pending_transfer =
        Transfer::new_pending(pending_id, acc1_id, acc2_id, Amount::new(2000), 4)
            .expect("valid pending transfer");

    let pending_res = cluster
        .propose(RaftRequest::CreatePending(pending_transfer))
        .await
        .expect("pending transfer hold should commit");
    assert_eq!(pending_res, RaftResponse::PendingCreated(pending_id));

    let post_res = cluster
        .propose(RaftRequest::PostPending {
            pending_id,
            post_transfer_id: post_id,
            amount: Amount::new(2000),
            timestamp: 5,
        })
        .await
        .expect("post pending hold should commit");
    assert_eq!(post_res, RaftResponse::PendingPosted(post_id));

    // Allow followers to apply committed entries
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Verify all 3 nodes have byte-for-byte identical balances and invariants
    for id in 1..=3 {
        let node = cluster.get_node(id).expect("node exists");
        node.read_ledger(|ledger| {
            ledger
                .verify_invariants()
                .expect("invariants must hold on all nodes");

            let acc1 = ledger.get_account(acc1_id).expect("acc1 exists");
            let acc2 = ledger.get_account(acc2_id).expect("acc2 exists");

            // Total debits = 50.00 + 20.00 = 70.00 (raw 7000)
            assert_eq!(acc1.balance.debits_posted.as_u128(), 7000);
            assert_eq!(acc2.balance.credits_posted.as_u128(), 7000);
        })
        .await;
    }

    cluster.shutdown().await;
}
