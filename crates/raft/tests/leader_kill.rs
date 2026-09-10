use std::time::Duration;

use ledger_core::{AccountFlags, AccountId, AccountType, Amount, Scale, Transfer, TransferId};
use raft::{RaftCluster, RaftRequest};
use tokio::time::{sleep, Instant};

#[tokio::test]
async fn test_leader_kill_and_rapid_failover() {
    let scale = Scale::new(2).expect("valid scale");
    let mut cluster = RaftCluster::new_3node(scale)
        .await
        .expect("cluster should bootstrap");

    let initial_leader = cluster
        .wait_for_leader(Duration::from_secs(3))
        .await
        .expect("cluster must elect a leader");

    let acc1_id = AccountId::new(301);
    let acc2_id = AccountId::new(302);

    cluster
        .propose(RaftRequest::CreateAccount {
            id: acc1_id,
            account_type: AccountType::Asset,
            flags: AccountFlags::bank_asset(),
            scale,
            timestamp: 1,
        })
        .await
        .expect("account 1 created");

    cluster
        .propose(RaftRequest::CreateAccount {
            id: acc2_id,
            account_type: AccountType::Liability,
            flags: AccountFlags::customer(),
            scale,
            timestamp: 2,
        })
        .await
        .expect("account 2 created");

    // Commit 1 transfer before killing the leader
    let t1 = Transfer::new_immediate(
        TransferId::new(4001),
        acc1_id,
        acc2_id,
        Amount::new(3000),
        3,
    )
    .expect("valid transfer");
    cluster
        .propose(RaftRequest::CreateTransfer(t1))
        .await
        .expect("transfer 1 committed");

    // KILL THE LEADER
    let kill_start = Instant::now();
    cluster
        .kill_node(initial_leader)
        .await
        .expect("leader killed");

    // Wait for the remaining 2 nodes to elect a new leader
    let mut new_leader_id = None;
    while kill_start.elapsed() < Duration::from_secs(2) {
        for (&id, node) in cluster.nodes() {
            if id != initial_leader && node.is_leader() {
                new_leader_id = Some(id);
                break;
            }
        }
        if new_leader_id.is_some() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }

    let failover_duration = kill_start.elapsed();
    let new_leader = new_leader_id.expect("a new leader must be elected within 2s failover SLO");
    assert_ne!(new_leader, initial_leader);
    assert!(
        failover_duration < Duration::from_secs(2),
        "failover took {failover_duration:?}, must be < 2.0s"
    );

    // Commit 2 additional transfers to the new leader
    let new_leader_node = cluster.get_node(new_leader).expect("new leader exists");

    for i in 2u64..=3u64 {
        let t = Transfer::new_immediate(
            TransferId::new(4000 + i as u128),
            acc1_id,
            acc2_id,
            Amount::new(1000),
            i + 3,
        )
        .expect("valid transfer");

        new_leader_node
            .client_write(RaftRequest::CreateTransfer(t))
            .await
            .expect("new leader must commit writes with remaining quorum");
    }

    // Verify both surviving nodes have identical balances and zero lost data
    // Total transferred: 3000 + (2 * 1000) = 5000
    for (&id, node) in cluster.nodes() {
        node.read_ledger(|ledger| {
            ledger
                .verify_invariants()
                .expect("invariants must hold after failover");

            let acc1 = ledger.get_account(acc1_id).expect("acc1 exists");
            let acc2 = ledger.get_account(acc2_id).expect("acc2 exists");

            assert_eq!(
                acc1.balance.debits_posted.as_u128(),
                5000,
                "node {id} balance must match"
            );
            assert_eq!(
                acc2.balance.credits_posted.as_u128(),
                5000,
                "node {id} balance must match"
            );
        })
        .await;
    }

    cluster.shutdown().await;
}
