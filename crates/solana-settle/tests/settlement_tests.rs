use ledger_core::amount::Amount;
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use merkle::{hash_transfer, MerkleMountainRange};
use solana_program::account_info::AccountInfo;
use solana_program::instruction::Instruction;
use solana_program::pubkey::Pubkey;
use solana_settle::client::TransferReceipt;
use solana_settle::instruction::{commit_settlement, initialize, verify_inclusion, CommitParams};
use solana_settle::processor::Processor;
use solana_settle::state::SettlementRoot;

/// Mock account structure for offline Solana runtime execution.
struct MockAccount {
    pub key: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: Pubkey,
}

impl MockAccount {
    fn new(
        key: Pubkey,
        is_signer: bool,
        is_writable: bool,
        lamports: u64,
        data: Vec<u8>,
        owner: Pubkey,
    ) -> Self {
        Self {
            key,
            is_signer,
            is_writable,
            lamports,
            data,
            owner,
        }
    }
}

/// Helper to execute an instruction through Processor with mock AccountInfo buffers.
fn execute_ix(
    program_id: &Pubkey,
    ix: &Instruction,
    accounts: &mut [MockAccount],
) -> Result<(), solana_program::program_error::ProgramError> {
    let mut account_infos = Vec::new();

    for acc in accounts.iter_mut() {
        account_infos.push(AccountInfo::new(
            &acc.key,
            acc.is_signer,
            acc.is_writable,
            &mut acc.lamports,
            &mut acc.data,
            &acc.owner,
            false,
            0,
        ));
    }

    Processor::process(program_id, &account_infos, &ix.data)
}

#[test]
fn test_settlement_lifecycle_and_invariants() {
    let program_id = Pubkey::new_unique();
    let authority = Pubkey::new_unique();
    let system_program = Pubkey::default();
    let clock_sysvar = Pubkey::default();

    let (pda, bump) = SettlementRoot::find_pda(&authority, &program_id);

    let mut pda_data = vec![0u8; SettlementRoot::LEN];
    let pda_lamports = 1_000_000;
    let auth_lamports = 10_000_000;
    let auth_data = vec![];
    let sys_lamports = 1;
    let sys_data = vec![];

    // 1. Initialize
    let init_ix = initialize(program_id, authority, system_program);
    let mut accounts = vec![
        MockAccount::new(
            authority,
            true,
            false,
            auth_lamports,
            auth_data.clone(),
            system_program,
        ),
        MockAccount::new(pda, false, true, pda_lamports, pda_data.clone(), program_id),
        MockAccount::new(
            system_program,
            false,
            false,
            sys_lamports,
            sys_data.clone(),
            system_program,
        ),
    ];

    execute_ix(&program_id, &init_ix, &mut accounts).expect("initialize should succeed");
    pda_data = accounts[1].data.clone();

    let root_state = SettlementRoot::deserialize_from(&pda_data).expect("deserialize state");
    assert_eq!(root_state.authority, authority);
    assert_eq!(root_state.batch_seq, 0);
    assert_eq!(root_state.bump, bump);

    // 2. Commit Batch 1
    let r1 = [0x11u8; 32];
    let tip1 = [0xaa; 32];
    let commit1_params = CommitParams {
        epoch: 1,
        batch_seq: 1,
        merkle_root: r1,
        previous_root: [0u8; 32],
        transfer_count: 50,
        total_settled_amount: 100_000,
        chain_tip: tip1,
    };
    let commit1_ix = commit_settlement(program_id, authority, commit1_params, clock_sysvar);
    let mut accounts = vec![
        MockAccount::new(
            authority,
            true,
            false,
            auth_lamports,
            auth_data.clone(),
            system_program,
        ),
        MockAccount::new(pda, false, true, pda_lamports, pda_data.clone(), program_id),
    ];

    execute_ix(&program_id, &commit1_ix, &mut accounts).expect("commit batch 1");
    pda_data = accounts[1].data.clone();

    let root_state = SettlementRoot::deserialize_from(&pda_data).expect("deserialize state");
    assert_eq!(root_state.batch_seq, 1);
    assert_eq!(root_state.merkle_root, r1);
    assert_eq!(root_state.total_settled_amount, 100_000);

    // 3. Reject non-sequential batch 3 (should be 2)
    let commit_bad_seq = CommitParams {
        epoch: 1,
        batch_seq: 3, // skipped batch 2!
        merkle_root: [0x33; 32],
        previous_root: r1,
        transfer_count: 10,
        total_settled_amount: 500,
        chain_tip: [0xbb; 32],
    };
    let commit_bad_ix = commit_settlement(program_id, authority, commit_bad_seq, clock_sysvar);
    let mut accounts = vec![
        MockAccount::new(
            authority,
            true,
            false,
            auth_lamports,
            auth_data.clone(),
            system_program,
        ),
        MockAccount::new(pda, false, true, pda_lamports, pda_data.clone(), program_id),
    ];
    assert!(execute_ix(&program_id, &commit_bad_ix, &mut accounts).is_err());

    // 4. Reject broken previous_root hash chain
    let commit_bad_prev = CommitParams {
        epoch: 1,
        batch_seq: 2,
        merkle_root: [0x22; 32],
        previous_root: [0x99; 32], // does not match r1!
        transfer_count: 10,
        total_settled_amount: 500,
        chain_tip: [0xbb; 32],
    };
    let commit_bad_prev_ix =
        commit_settlement(program_id, authority, commit_bad_prev, clock_sysvar);
    let mut accounts = vec![
        MockAccount::new(
            authority,
            true,
            false,
            auth_lamports,
            auth_data.clone(),
            system_program,
        ),
        MockAccount::new(pda, false, true, pda_lamports, pda_data.clone(), program_id),
    ];
    assert!(execute_ix(&program_id, &commit_bad_prev_ix, &mut accounts).is_err());

    // 5. Commit Batch 2 successfully with proper previous_root
    let r2 = [0x22u8; 32];
    let commit2_params = CommitParams {
        epoch: 1,
        batch_seq: 2,
        merkle_root: r2,
        previous_root: r1,
        transfer_count: 25,
        total_settled_amount: 50_000,
        chain_tip: [0xcc; 32],
    };
    let commit2_ix = commit_settlement(program_id, authority, commit2_params, clock_sysvar);
    let mut accounts = vec![
        MockAccount::new(
            authority,
            true,
            false,
            auth_lamports,
            auth_data.clone(),
            system_program,
        ),
        MockAccount::new(pda, false, true, pda_lamports, pda_data.clone(), program_id),
    ];
    execute_ix(&program_id, &commit2_ix, &mut accounts).expect("commit batch 2");
    pda_data = accounts[1].data.clone();

    let root_state = SettlementRoot::deserialize_from(&pda_data).expect("deserialize state");
    assert_eq!(root_state.batch_seq, 2);
    assert_eq!(root_state.merkle_root, r2);
    assert_eq!(root_state.previous_root, r1);
    assert_eq!(root_state.total_settled_amount, 150_000);
}

#[test]
fn test_on_chain_merkle_proof_verification() {
    let program_id = Pubkey::new_unique();
    let authority = Pubkey::new_unique();
    let (pda, bump) = SettlementRoot::find_pda(&authority, &program_id);

    // Build MMR with 5 real transfers
    let mut mmr = MerkleMountainRange::new();
    let mut transfers = Vec::new();

    for i in 1..=5 {
        let t = Transfer::new_immediate(
            TransferId::new(i),
            AccountId::new(100 + i),
            AccountId::new(200 + i),
            Amount::new(i * 100),
            1_700_000_000 + i as u64,
        )
        .expect("valid transfer");

        mmr.append_transfer(&t).expect("append transfer");
        transfers.push(t);
    }

    let batch_root = mmr.root();

    // Initialize PDA with this batch committed
    let mut pda_data = vec![0u8; SettlementRoot::LEN];
    let state = SettlementRoot {
        authority,
        epoch: 1,
        batch_seq: 1,
        merkle_root: *batch_root.as_bytes(),
        previous_root: [0u8; 32],
        transfer_count: 5,
        total_settled_amount: 1500,
        chain_tip: [0x55; 32],
        settled_at: 1_700_000_100,
        bump,
    };
    state
        .serialize_into(&mut pda_data)
        .expect("serialize state");

    // Generate proof for transfer index 2 (transfer ID 3)
    let target_idx = 2;
    let target_transfer = &transfers[target_idx];
    let target_leaf_hash = hash_transfer(target_transfer).expect("hash transfer");
    let proof = mmr.generate_proof(target_idx).expect("generate proof");
    let proof_bytes = proof.to_bytes().expect("serialize proof");

    // 1. Verify valid proof on-chain
    let verify_ix = verify_inclusion(
        program_id,
        authority,
        *target_leaf_hash.as_bytes(),
        proof_bytes.clone(),
    );
    let mut accounts = vec![MockAccount::new(
        pda,
        false,
        false,
        1_000_000,
        pda_data.clone(),
        program_id,
    )];
    assert!(execute_ix(&program_id, &verify_ix, &mut accounts).is_ok());

    // 2. Reject tampered leaf hash on-chain
    let mut forged_leaf = *target_leaf_hash.as_bytes();
    forged_leaf[0] ^= 0xff;
    let bad_verify_ix = verify_inclusion(program_id, authority, forged_leaf, proof_bytes);
    let mut accounts = vec![MockAccount::new(
        pda,
        false,
        false,
        1_000_000,
        pda_data.clone(),
        program_id,
    )];
    assert!(execute_ix(&program_id, &bad_verify_ix, &mut accounts).is_err());
}

#[test]
fn test_transfer_receipt_file_roundtrip() {
    let mut mmr = MerkleMountainRange::new();
    let t = Transfer::new_immediate(
        TransferId::new(9999),
        AccountId::new(500),
        AccountId::new(600),
        Amount::new(75_000),
        1_700_000_500,
    )
    .expect("valid transfer");

    mmr.append_transfer(&t).expect("append transfer");
    let leaf_hash = hash_transfer(&t).expect("hash transfer");
    let proof = mmr.generate_proof(0).expect("proof");

    let receipt = TransferReceipt {
        transfer_id: 9999,
        amount: 75_000,
        debit_account: 500,
        credit_account: 600,
        leaf_hash: leaf_hash.to_hex(),
        batch_seq: 42,
        merkle_root: mmr.root().to_hex(),
        proof,
    };

    assert!(receipt.verify().is_ok());

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file_path = temp_dir.path().join("receipt.json");

    receipt.save_to_file(&file_path).expect("save receipt");
    let loaded = TransferReceipt::load_from_file(&file_path).expect("load receipt");

    assert_eq!(loaded, receipt);
    assert!(loaded.verify().is_ok());
}

#[test]
fn test_verifier_binary_cli_execution() {
    let mut mmr = MerkleMountainRange::new();
    let t = Transfer::new_immediate(
        TransferId::new(777),
        AccountId::new(10),
        AccountId::new(20),
        Amount::new(30_000),
        1_700_000_777,
    )
    .expect("valid transfer");

    mmr.append_transfer(&t).expect("append transfer");
    let leaf_hash = hash_transfer(&t).expect("hash transfer");
    let proof = mmr.generate_proof(0).expect("proof");

    let receipt = TransferReceipt {
        transfer_id: 777,
        amount: 30_000,
        debit_account: 10,
        credit_account: 20,
        leaf_hash: leaf_hash.to_hex(),
        batch_seq: 1,
        merkle_root: mmr.root().to_hex(),
        proof,
    };

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let valid_receipt_path = temp_dir.path().join("valid_receipt.json");
    receipt
        .save_to_file(&valid_receipt_path)
        .expect("save receipt");

    // Run CLI on valid receipt
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_verifier"))
        .arg("--receipt")
        .arg(&valid_receipt_path)
        .output()
        .expect("execute verifier binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[VERIFIED]"));

    // Run CLI on tampered receipt
    let mut tampered = receipt;
    tampered.merkle_root = "00".repeat(32);
    let tampered_receipt_path = temp_dir.path().join("tampered_receipt.json");
    tampered
        .save_to_file(&tampered_receipt_path)
        .expect("save tampered");

    let output_bad = std::process::Command::new(env!("CARGO_BIN_EXE_verifier"))
        .arg("--receipt")
        .arg(&tampered_receipt_path)
        .output()
        .expect("execute verifier binary");

    assert!(!output_bad.status.success());
    let stderr = String::from_utf8_lossy(&output_bad.stderr);
    assert!(stderr.contains("[FAILED]"));
}
