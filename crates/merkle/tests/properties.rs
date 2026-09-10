//! Comprehensive unit and property tests for Merkle Mountain Range and inclusion proofs.

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::journal::LedgerEvent;
use ledger_core::transfer::Transfer;
use merkle::{
    hash_leaf, hash_transfer, Hash32, HexError, MerkleMountainRange, MmrError, MmrProof, ProofError,
};
use proptest::prelude::*;

#[test]
fn test_hex_conversion_roundtrip() {
    let raw = [42u8; 32];
    let hash = Hash32::new(raw);
    let hex = hash.to_hex();
    assert_eq!(hex.len(), 64);
    let parsed = Hash32::from_hex(&hex).expect("valid hex");
    assert_eq!(parsed, hash);

    // Test invalid length
    assert_eq!(Hash32::from_hex("abcd"), Err(HexError::InvalidLength(4)));

    // Test invalid character
    let bad_hex = "zz".to_string() + &hex[2..];
    match Hash32::from_hex(&bad_hex) {
        Err(HexError::InvalidCharacter {
            char: 'z',
            index: 0,
        }) => {}
        other => panic!("expected invalid character error, got {other:?}"),
    }
}

#[test]
fn test_empty_mmr() {
    let mmr = MerkleMountainRange::new();
    assert!(mmr.is_empty());
    assert_eq!(mmr.leaf_count(), 0);
    assert_eq!(mmr.node_count(), 0);
    assert_eq!(mmr.root(), Hash32::ZERO);
    assert_eq!(mmr.peaks(), Vec::<Hash32>::new());

    assert_eq!(
        mmr.generate_proof(0),
        Err(MmrError::LeafIndexOutOfBounds { index: 0, count: 0 })
    );
}

#[test]
fn test_single_leaf_mmr() {
    let mut mmr = MerkleMountainRange::new();
    let leaf_data = b"transfer-001";
    let leaf_hash = hash_leaf(leaf_data);

    let idx = mmr.append_leaf(leaf_data);
    assert_eq!(idx, 0);
    assert_eq!(mmr.leaf_count(), 1);
    assert_eq!(mmr.node_count(), 1);
    assert_eq!(mmr.root(), leaf_hash);
    assert_eq!(mmr.peaks(), vec![leaf_hash]);

    let proof = mmr.generate_proof(0).expect("proof generation");
    assert_eq!(proof.leaf_index, 0);
    assert_eq!(proof.leaf_count, 1);
    assert!(proof.siblings.is_empty());
    assert_eq!(proof.peaks, vec![leaf_hash]);

    assert!(proof.verify(&mmr.root(), &leaf_hash).is_ok());
}

#[test]
fn test_deterministic_transfer_proof() {
    let mut mmr = MerkleMountainRange::new();

    let t1 = Transfer::new_immediate(
        TransferId::new(1),
        AccountId::new(10),
        AccountId::new(20),
        Amount::new(500),
        1_700_000_000,
    )
    .expect("valid transfer");

    let t2 = Transfer::new_immediate(
        TransferId::new(2),
        AccountId::new(30),
        AccountId::new(40),
        Amount::new(1_200),
        1_700_000_001,
    )
    .expect("valid transfer");

    let idx1 = mmr.append_transfer(&t1).expect("append transfer 1");
    let idx2 = mmr.append_transfer(&t2).expect("append transfer 2");

    assert_eq!(idx1, 0);
    assert_eq!(idx2, 1);
    assert_eq!(mmr.leaf_count(), 2);

    let t1_hash = hash_transfer(&t1).expect("hash transfer 1");
    let proof1 = mmr.generate_proof(0).expect("generate proof 1");
    assert!(proof1.verify(&mmr.root(), &t1_hash).is_ok());

    // Proof serialization roundtrip
    let proof_bytes = proof1.to_bytes().expect("serialize proof");
    let deserialized = MmrProof::from_bytes(&proof_bytes).expect("deserialize proof");
    assert_eq!(deserialized, proof1);
    assert!(deserialized.verify(&mmr.root(), &t1_hash).is_ok());

    // Tamper with transfer payload: verification must fail
    let tampered_t1 = Transfer::new_immediate(
        TransferId::new(1),
        AccountId::new(10),
        AccountId::new(20),
        Amount::new(999_999), // forged amount
        1_700_000_000,
    )
    .expect("valid transfer");
    let tampered_hash = hash_transfer(&tampered_t1).expect("hash tampered");
    match proof1.verify(&mmr.root(), &tampered_hash) {
        Err(ProofError::PeakMismatch { .. }) => {}
        other => panic!("expected PeakMismatch, got {other:?}"),
    }
}

#[test]
fn test_ledger_event_append_and_proof() {
    let mut mmr = MerkleMountainRange::new();

    let event = LedgerEvent::AccountCreated {
        id: AccountId::new(100),
        account_type: AccountType::Liability,
        flags: AccountFlags::customer(),
        scale: Scale::usd(),
        timestamp: 1_700_000_000,
    };

    let idx = mmr.append_ledger_event(&event).expect("append event");
    assert_eq!(idx, 0);

    let event_hash = mmr.leaf_hash(0).expect("leaf hash");
    let proof = mmr.generate_proof(0).expect("proof");
    assert!(proof.verify(&mmr.root(), &event_hash).is_ok());
}

#[test]
fn test_multi_leaf_powers_of_two_and_odd() {
    for leaf_count in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 64] {
        let mut mmr = MerkleMountainRange::new();
        let mut leaf_hashes = Vec::new();

        for i in 0..leaf_count {
            let data = format!("entry-{leaf_count}-{i}");
            let hash = hash_leaf(data.as_bytes());
            leaf_hashes.push(hash);
            mmr.append_leaf_hash(hash);
        }

        assert_eq!(mmr.leaf_count(), leaf_count);
        let root = mmr.root();

        // Verify proof for every single leaf
        for (i, leaf_hash) in leaf_hashes.iter().enumerate() {
            let proof = mmr.generate_proof(i).expect("proof generation");
            assert!(
                proof.verify(&root, leaf_hash).is_ok(),
                "failed to verify leaf {i} for total count {leaf_count}"
            );

            // Verify that an invalid root rejects
            let fake_root = Hash32::new([0xff; 32]);
            assert!(matches!(
                proof.verify(&fake_root, leaf_hash),
                Err(ProofError::RootMismatch { .. })
            ));
        }
    }
}

#[test]
fn test_tampered_siblings_rejected() {
    let mut mmr = MerkleMountainRange::new();
    for i in 0..10 {
        mmr.append_leaf(format!("leaf-{i}").as_bytes());
    }

    let leaf_hash = mmr.leaf_hash(3).expect("leaf hash");
    let mut proof = mmr.generate_proof(3).expect("proof");

    assert!(proof.verify(&mmr.root(), &leaf_hash).is_ok());

    // Tamper with sibling
    if !proof.siblings.is_empty() {
        proof.siblings[0].0[0] ^= 0x01;
        assert!(proof.verify(&mmr.root(), &leaf_hash).is_err());
    }
}

#[test]
fn test_mmr_state_serialization() {
    let mut mmr = MerkleMountainRange::new();
    for i in 0..25 {
        mmr.append_leaf(format!("record-{i}").as_bytes());
    }

    let bytes = mmr.to_bytes().expect("serialize MMR");
    let restored = MerkleMountainRange::from_bytes(&bytes).expect("deserialize MMR");

    assert_eq!(mmr.leaf_count(), restored.leaf_count());
    assert_eq!(mmr.root(), restored.root());
    assert_eq!(mmr.peaks(), restored.peaks());

    let leaf_hash = restored.leaf_hash(12).expect("leaf hash");
    let proof = restored.generate_proof(12).expect("proof");
    assert!(proof.verify(&restored.root(), &leaf_hash).is_ok());
}

proptest! {
    #[test]
    fn prop_mmr_any_leaf_count_inclusion(
        leaf_count in 1usize..=100,
        tamper_bit in 0u8..8,
    ) {
        let mut mmr = MerkleMountainRange::new();
        let mut leaves = Vec::with_capacity(leaf_count);

        for i in 0..leaf_count {
            let data = format!("prop-data-{i}");
            let hash = hash_leaf(data.as_bytes());
            leaves.push(hash);
            mmr.append_leaf_hash(hash);
        }

        let root = mmr.root();

        // Check inclusion for a sample of leaves
        for (i, &leaf_hash) in leaves.iter().enumerate() {
            let proof = mmr.generate_proof(i).expect("valid proof");

            // Legitimate proof must verify
            prop_assert!(proof.verify(&root, &leaf_hash).is_ok());

            // Tampered leaf hash must fail
            let mut bad_leaf = leaf_hash;
            bad_leaf.0[0] ^= 1 << tamper_bit;
            prop_assert!(proof.verify(&root, &bad_leaf).is_err());
        }
    }
}
