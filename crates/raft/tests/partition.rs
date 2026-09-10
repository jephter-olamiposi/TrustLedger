use std::time::{Duration, Instant};

use ledger_core::{AccountFlags, AccountId, AccountType, Amount, Scale, Transfer, TransferId};
use raft::{RaftCluster, RaftRequest};
use tokio::time::sleep;

#[tokio::test]
async fn test_jepsen_minority_partition_and_healing() {
    let scale = Scale::new(2).expect("valid scale");
    let mut cluster = RaftCluster::new_3node(scale)
        .await
        .expect("cluster should bootstrap");

    let initial_leader = cluster
        .wait_for_leader(Duration::from_secs(3))
        .await
        .expect("cluster must elect a leader");

    let acc1_id = AccountId::new(201);
    let acc2_id = AccountId::new(202);

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

    // Seed 1 committed transfer
    let t1 = Transfer::new_immediate(
        TransferId::new(3001),
        acc1_id,
        acc2_id,
        Amount::new(1000),
        3,
    )
    .expect("valid transfer");
    cluster
        .propose(RaftRequest::CreateTransfer(t1))
        .await
        .expect("transfer 1 committed");

    // Identify minority and majority partition sets
    let minority = vec![initial_leader];
    let majority: Vec<u64> = (1..=3).filter(|&id| id != initial_leader).collect();
    assert_eq!(majority.len(), 2);

    // INJECT PARTITION: isolate initial leader from majority
    cluster.partition(&minority, &majority).await;

    // Wait for the majority partition to detect heartbeat loss and elect a new leader
    // (election timeout is 150-300ms, allow up to 2s under heavy test runner load)
    let elect_start = Instant::now();
    let mut new_leader_id = None;
    while elect_start.elapsed() < Duration::from_secs(2) {
        for &id in &majority {
            if let Some(node) = cluster.get_node(id) {
                if node.is_leader() {
                    new_leader_id = Some(id);
                    break;
                }
            }
        }
        if new_leader_id.is_some() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }

    let new_leader_id = new_leader_id.expect("majority partition must elect a new leader");
    assert_ne!(new_leader_id, initial_leader);

    // Commit 3 transfers to the new leader on the majority partition
    let new_leader_node = cluster.get_node(new_leader_id).expect("leader node exists");

    for i in 2u64..=4u64 {
        let t = Transfer::new_immediate(
            TransferId::new(3000 + i as u128),
            acc1_id,
            acc2_id,
            Amount::new(500),
            i + 3,
        )
        .expect("valid transfer");

        new_leader_node
            .client_write(RaftRequest::CreateTransfer(t))
            .await
            .expect("majority partition must commit writes without minority");
    }

    // HEAL PARTITION: reconnect minority node to majority
    cluster.heal().await;

    // Allow cluster to synchronize and minority to catch up
    let heal_start = Instant::now();
    while heal_start.elapsed() < Duration::from_secs(2) {
        let mut all_caught_up = true;
        for id in 1..=3 {
            if let Some(node) = cluster.get_node(id) {
                let debits = node
                    .read_ledger(|l| {
                        l.get_account(acc1_id)
                            .map(|a| a.balance.debits_posted.as_u128())
                            .unwrap_or(0)
                    })
                    .await;
                if debits != 2500 {
                    all_caught_up = false;
                    break;
                }
            }
        }
        if all_caught_up {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }

    // Verify all 3 nodes have converged to identical state:
    // Total transferred: 1000 + (3 * 500) = 2500
    for id in 1..=3 {
        let node = cluster.get_node(id).expect("node exists");
        node.read_ledger(|ledger| {
            ledger
                .verify_invariants()
                .expect("invariants must hold after partition heal");

            let acc1 = ledger.get_account(acc1_id).expect("acc1 exists");
            let acc2 = ledger.get_account(acc2_id).expect("acc2 exists");

            assert_eq!(
                acc1.balance.debits_posted.as_u128(),
                2500,
                "node {id} balance must match"
            );
            assert_eq!(
                acc2.balance.credits_posted.as_u128(),
                2500,
                "node {id} balance must match"
            );
        })
        .await;
    }

    cluster.shutdown().await;
}
