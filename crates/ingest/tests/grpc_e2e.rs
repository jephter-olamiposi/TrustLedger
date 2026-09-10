//! End-to-end gRPC integration test suite for TrustLedger ingest.

use std::time::Duration;

use ingest::proto::ledger_service_client::LedgerServiceClient;
use ingest::proto::ledger_service_server::LedgerServiceServer;
use ingest::proto::{
    AccountFlags, AccountType, Amount, ApplyBatchRequest, BatchOperation, CreateAccountRequest,
    CreatePendingRequest, CreateTransferRequest, GetAccountRequest, GetTransferRequest,
    PostPendingRequest, TransferState, VoidPendingRequest,
};
use ingest::{Engine, EngineConfig, LedgerServer};
use ledger_core::amount::Scale;
use ledger_core::Ledger;
use tokio_stream::wrappers::TcpListenerStream;
use wal::{Wal, WalOptions};

struct TestEnv {
    addr: std::net::SocketAddr,
    _dir: tempfile::TempDir,
}

async fn start_test_server(queue_capacity: usize) -> TestEnv {
    let dir = tempfile::tempdir().expect("tempdir");
    let wal_path = dir.path().join("wal.log");
    let (wal, _) = Wal::open(&wal_path, WalOptions::default()).expect("open wal");
    let ledger = Ledger::new(Scale::usdc());
    let config = EngineConfig {
        max_batch_size: 64,
        max_batch_delay: Duration::from_millis(1),
    };
    let (mut engine, queue) = Engine::new(ledger, wal, config, queue_capacity);
    tokio::spawn(async move {
        let _ = engine.run().await;
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let server = LedgerServer::new(queue, Scale::usdc());

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(LedgerServiceServer::new(server))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });

    TestEnv { addr, _dir: dir }
}

#[tokio::test]
async fn grpc_lifecycle_and_error_matrix() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    let bank_resp = client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: false,
                credits_must_not_exceed_debits: true,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 1,
        })
        .await
        .expect("create bank account")
        .into_inner();
    let bank = bank_resp.account.expect("bank account");
    assert_eq!(bank.id, 1);
    assert_eq!(bank.account_type, AccountType::Asset as i32);

    let alice_resp = client
        .create_account(CreateAccountRequest {
            id: 2,
            account_type: AccountType::Liability as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: true,
                credits_must_not_exceed_debits: false,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 2,
        })
        .await
        .expect("create alice account")
        .into_inner();
    assert_eq!(alice_resp.account.unwrap().id, 2);

    client
        .create_account(CreateAccountRequest {
            id: 3,
            account_type: AccountType::Liability as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: true,
                credits_must_not_exceed_debits: false,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 3,
        })
        .await
        .expect("create merchant account");

    let fund_resp = client
        .create_transfer(CreateTransferRequest {
            id: 100,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 10_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 4,
        })
        .await
        .expect("fund alice")
        .into_inner();
    let fund_transfer = fund_resp.transfer.expect("transfer");
    assert_eq!(fund_transfer.state, TransferState::Posted as i32);

    let pay_resp = client
        .create_transfer(CreateTransferRequest {
            id: 101,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 4_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 5,
        })
        .await
        .expect("alice pays merchant")
        .into_inner();
    let pay_transfer = pay_resp.transfer.expect("transfer");
    assert_eq!(pay_transfer.state, TransferState::Posted as i32);

    let alice_acc = client
        .get_account(GetAccountRequest { id: 2 })
        .await
        .expect("get alice")
        .into_inner()
        .account
        .unwrap();
    let alice_bal = alice_acc.balance.unwrap();
    assert_eq!(alice_bal.debits_posted.unwrap().units, 4_000_000);
    assert_eq!(alice_bal.credits_posted.unwrap().units, 10_000_000);

    let hold_resp = client
        .create_pending(CreatePendingRequest {
            id: 200,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 2_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 6,
        })
        .await
        .expect("hold")
        .into_inner();
    assert_eq!(
        hold_resp.transfer.unwrap().state,
        TransferState::Pending as i32
    );

    let post_resp = client
        .post_pending(PostPendingRequest {
            pending_id: 200,
            post_transfer_id: 201,
            amount: Some(Amount {
                units: 2_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 7,
        })
        .await
        .expect("post pending")
        .into_inner();
    assert_eq!(
        post_resp.transfer.unwrap().state,
        TransferState::Posted as i32
    );

    let get_post = client
        .get_transfer(GetTransferRequest { id: 201 })
        .await
        .expect("get posted transfer")
        .into_inner()
        .transfer
        .unwrap();
    assert_eq!(get_post.state, TransferState::Posted as i32);
    assert_eq!(get_post.pending_id, Some(200));

    client
        .create_pending(CreatePendingRequest {
            id: 300,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 1_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 8,
        })
        .await
        .expect("hold 2");

    let void_resp = client
        .void_pending(VoidPendingRequest {
            pending_id: 300,
            timestamp: 9,
        })
        .await
        .expect("void pending")
        .into_inner();
    assert_eq!(
        void_resp.transfer.unwrap().state,
        TransferState::Voided as i32
    );

    let get_void = client
        .get_transfer(GetTransferRequest { id: 300 })
        .await
        .expect("get voided transfer")
        .into_inner()
        .transfer
        .unwrap();
    assert_eq!(get_void.state, TransferState::Voided as i32);

    let err_funds = client
        .create_transfer(CreateTransferRequest {
            id: 400,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 999_999_999,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 10,
        })
        .await
        .expect_err("insufficient funds must fail");
    assert_eq!(err_funds.code(), tonic::Code::FailedPrecondition);

    let err_not_found = client
        .create_transfer(CreateTransferRequest {
            id: 401,
            debit_account_id: 99999,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 100,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 11,
        })
        .await
        .expect_err("account not found must fail");
    assert_eq!(err_not_found.code(), tonic::Code::NotFound);

    let err_dup_acc = client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: None,
            scale: 6,
            timestamp: 12,
        })
        .await
        .expect_err("duplicate account must fail");
    assert_eq!(err_dup_acc.code(), tonic::Code::AlreadyExists);

    let err_dup_xfer = client
        .create_transfer(CreateTransferRequest {
            id: 100,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 100,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 13,
        })
        .await
        .expect_err("duplicate transfer must fail");
    assert_eq!(err_dup_xfer.code(), tonic::Code::AlreadyExists);

    let batch_resp = client
        .apply_batch(ApplyBatchRequest {
            operations: vec![
                BatchOperation {
                    operation: Some(ingest::proto::batch_operation::Operation::CreateAccount(
                        CreateAccountRequest {
                            id: 10,
                            account_type: AccountType::Liability as i32,
                            flags: None,
                            scale: 6,
                            timestamp: 14,
                        },
                    )),
                },
                BatchOperation {
                    operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                        CreateTransferRequest {
                            id: 500,
                            debit_account_id: 1,
                            credit_account_id: 10,
                            amount: Some(Amount {
                                units: 500_000,
                                scale: 6,
                                units_high: 0,
                            }),
                            timestamp: 15,
                        },
                    )),
                },
            ],
        })
        .await
        .expect("apply batch")
        .into_inner();
    assert_eq!(batch_resp.applied_count, 2);
}

#[tokio::test]
async fn test_fifo_causality_apply_batch_then_transfer() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: false,
                credits_must_not_exceed_debits: true,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 1,
        })
        .await
        .expect("create bank");

    client
        .create_account(CreateAccountRequest {
            id: 2,
            account_type: AccountType::Liability as i32,
            flags: None,
            scale: 6,
            timestamp: 2,
        })
        .await
        .expect("create merchant");

    let mut client2 = client.clone();
    let batch_future = client.apply_batch(ApplyBatchRequest {
        operations: vec![
            BatchOperation {
                operation: Some(ingest::proto::batch_operation::Operation::CreateAccount(
                    CreateAccountRequest {
                        id: 77,
                        account_type: AccountType::Liability as i32,
                        flags: Some(AccountFlags {
                            debits_must_not_exceed_credits: true,
                            credits_must_not_exceed_debits: false,
                            is_closed: false,
                        }),
                        scale: 6,
                        timestamp: 3,
                    },
                )),
            },
            BatchOperation {
                operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                    CreateTransferRequest {
                        id: 770,
                        debit_account_id: 1,
                        credit_account_id: 77,
                        amount: Some(Amount {
                            units: 2_000_000,
                            scale: 6,
                            units_high: 0,
                        }),
                        timestamp: 4,
                    },
                )),
            },
        ],
    });

    // Invariant: FIFO execution ensures account 77 is committed before subsequent transfer runs.
    let transfer_future = client2.create_transfer(CreateTransferRequest {
        id: 771,
        debit_account_id: 77,
        credit_account_id: 2,
        amount: Some(Amount {
            units: 500_000,
            scale: 6,
            units_high: 0,
        }),
        timestamp: 5,
    });

    let (batch_res, transfer_res) = tokio::join!(batch_future, transfer_future);

    assert_eq!(
        batch_res.expect("apply batch").into_inner().applied_count,
        2
    );
    let xfer = transfer_res
        .expect("transfer from newly created account")
        .into_inner()
        .transfer
        .unwrap();
    assert_eq!(xfer.id, 771);
    assert_eq!(xfer.state, TransferState::Posted as i32);

    // Verify account 77 balances
    let acc77 = client
        .get_account(GetAccountRequest { id: 77 })
        .await
        .expect("get account 77")
        .into_inner()
        .account
        .unwrap();
    let bal77 = acc77.balance.unwrap();
    assert_eq!(bal77.credits_posted.unwrap().units, 2_000_000);
    assert_eq!(bal77.debits_posted.unwrap().units, 500_000);
}

#[tokio::test]
async fn test_auto_timestamp_allocation_and_monotonicity() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: None,
            scale: 6,
            timestamp: 1_000_000,
        })
        .await
        .expect("create bank");

    let cust_resp = client
        .create_account(CreateAccountRequest {
            id: 2,
            account_type: AccountType::Liability as i32,
            flags: None,
            scale: 6,
            timestamp: 0,
        })
        .await
        .expect("create customer with 0 timestamp")
        .into_inner();
    let cust_acc = cust_resp.account.unwrap();
    assert_eq!(cust_acc.id, 2);

    let xfer_resp = client
        .create_transfer(CreateTransferRequest {
            id: 10,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 1_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
        .expect("create transfer with 0 timestamp")
        .into_inner();
    let xfer = xfer_resp.transfer.unwrap();
    assert!(
        xfer.timestamp > 1_000_000,
        "auto-timestamp must exceed prior watermark of 1_000_000, got {}",
        xfer.timestamp
    );

    let pending_resp = client
        .create_pending(CreatePendingRequest {
            id: 20,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 500_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
        .expect("create pending with 0 timestamp")
        .into_inner();
    let pending = pending_resp.transfer.unwrap();
    assert!(
        pending.timestamp > xfer.timestamp,
        "pending timestamp {} must exceed previous transfer timestamp {}",
        pending.timestamp,
        xfer.timestamp
    );

    let post_resp = client
        .post_pending(PostPendingRequest {
            pending_id: 20,
            post_transfer_id: 21,
            amount: Some(Amount {
                units: 500_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
        .expect("post pending with 0 timestamp")
        .into_inner();
    let posted = post_resp.transfer.unwrap();
    assert!(
        posted.timestamp > pending.timestamp,
        "posted timestamp {} must exceed pending timestamp {}",
        posted.timestamp,
        pending.timestamp
    );
}

#[tokio::test]
async fn test_apply_batch_with_zero_timestamps() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: None,
            scale: 6,
            timestamp: 500_000,
        })
        .await
        .expect("seed account");

    let batch_resp = client
        .apply_batch(ApplyBatchRequest {
            operations: vec![
                BatchOperation {
                    operation: Some(ingest::proto::batch_operation::Operation::CreateAccount(
                        CreateAccountRequest {
                            id: 2,
                            account_type: AccountType::Liability as i32,
                            flags: None,
                            scale: 6,
                            timestamp: 0,
                        },
                    )),
                },
                BatchOperation {
                    operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                        CreateTransferRequest {
                            id: 100,
                            debit_account_id: 1,
                            credit_account_id: 2,
                            amount: Some(Amount {
                                units: 100_000,
                                scale: 6,
                                units_high: 0,
                            }),
                            timestamp: 0,
                        },
                    )),
                },
            ],
        })
        .await
        .expect("apply batch with zero timestamps must succeed")
        .into_inner();
    assert_eq!(batch_resp.applied_count, 2);

    let xfer = client
        .get_transfer(GetTransferRequest { id: 100 })
        .await
        .expect("get transfer")
        .into_inner()
        .transfer
        .unwrap();
    assert!(
        xfer.timestamp > 500_000,
        "batch transfer timestamp {} must exceed 500_000",
        xfer.timestamp
    );
}

#[tokio::test]
async fn test_scale_mismatch_rejected() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: None,
            scale: 6,
            timestamp: 1,
        })
        .await
        .expect("seed 1");
    client
        .create_account(CreateAccountRequest {
            id: 2,
            account_type: AccountType::Liability as i32,
            flags: None,
            scale: 6,
            timestamp: 2,
        })
        .await
        .expect("seed 2");

    let err = client
        .create_transfer(CreateTransferRequest {
            id: 10,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 1000,
                scale: 2,
                units_high: 0,
            }),
            timestamp: 3,
        })
        .await
        .expect_err("scale mismatch must be rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let err_scale_overflow = client
        .create_transfer(CreateTransferRequest {
            id: 11,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 1000,
                scale: 500,
                units_high: 0,
            }),
            timestamp: 4,
        })
        .await
        .expect_err("scale exceeding u8 must be rejected");
    assert_eq!(err_scale_overflow.code(), tonic::Code::InvalidArgument);
}

#[tokio::test]
async fn test_intra_batch_failure_isolation_and_concurrent_group_commit() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: false,
                credits_must_not_exceed_debits: true,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 1,
        })
        .await
        .expect("bank");

    client
        .create_account(CreateAccountRequest {
            id: 2,
            account_type: AccountType::Liability as i32,
            flags: Some(AccountFlags {
                debits_must_not_exceed_credits: true,
                credits_must_not_exceed_debits: false,
                is_closed: false,
            }),
            scale: 6,
            timestamp: 2,
        })
        .await
        .expect("customer");

    client
        .create_account(CreateAccountRequest {
            id: 3,
            account_type: AccountType::Liability as i32,
            flags: None,
            scale: 6,
            timestamp: 3,
        })
        .await
        .expect("merchant");

    client
        .create_transfer(CreateTransferRequest {
            id: 10,
            debit_account_id: 1,
            credit_account_id: 2,
            amount: Some(Amount {
                units: 10_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 4,
        })
        .await
        .expect("fund customer");

    let url = format!("http://{}", env.addr);
    let mut c1 = LedgerServiceClient::connect(url.clone()).await.unwrap();
    let mut c2 = LedgerServiceClient::connect(url.clone()).await.unwrap();
    let mut c3 = LedgerServiceClient::connect(url.clone()).await.unwrap();
    let mut c4 = LedgerServiceClient::connect(url.clone()).await.unwrap();
    let mut c5 = LedgerServiceClient::connect(url.clone()).await.unwrap();

    let h1 = tokio::spawn(async move {
        c1.create_transfer(CreateTransferRequest {
            id: 101,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 2_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
    });

    let h2 = tokio::spawn(async move {
        c2.create_transfer(CreateTransferRequest {
            id: 102,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 50_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
    });

    let h3 = tokio::spawn(async move {
        c3.create_transfer(CreateTransferRequest {
            id: 103,
            debit_account_id: 2,
            credit_account_id: 3,
            amount: Some(Amount {
                units: 3_000_000,
                scale: 6,
                units_high: 0,
            }),
            timestamp: 0,
        })
        .await
    });

    let h4 = tokio::spawn(async move {
        c4.apply_batch(ApplyBatchRequest {
            operations: vec![BatchOperation {
                operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                    CreateTransferRequest {
                        id: 104,
                        debit_account_id: 2,
                        credit_account_id: 3,
                        amount: Some(Amount {
                            units: 99_000_000,
                            scale: 6,
                            units_high: 0,
                        }),
                        timestamp: 0,
                    },
                )),
            }],
        })
        .await
    });

    let h5 = tokio::spawn(async move {
        c5.apply_batch(ApplyBatchRequest {
            operations: vec![BatchOperation {
                operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                    CreateTransferRequest {
                        id: 105,
                        debit_account_id: 2,
                        credit_account_id: 3,
                        amount: Some(Amount {
                            units: 1_000_000,
                            scale: 6,
                            units_high: 0,
                        }),
                        timestamp: 0,
                    },
                )),
            }],
        })
        .await
    });

    let (r1, r2, r3, r4, r5) = tokio::join!(h1, h2, h3, h4, h5);
    let res1 = r1.unwrap();
    let res2 = r2.unwrap();
    let res3 = r3.unwrap();
    let res4 = r4.unwrap();
    let res5 = r5.unwrap();

    assert!(
        res1.is_ok(),
        "valid transfer 101 must succeed, got {:?}",
        res1.err()
    );
    assert_eq!(
        res2.expect_err("overdraft transfer must fail").code(),
        tonic::Code::FailedPrecondition
    );
    assert!(res3.is_ok(), "valid transfer 103 must succeed");
    assert_eq!(
        res4.expect_err("atomic batch with overdraft must fail")
            .code(),
        tonic::Code::FailedPrecondition
    );
    assert!(res5.is_ok(), "valid batch 105 must succeed");

    let cust = client
        .get_account(GetAccountRequest { id: 2 })
        .await
        .unwrap()
        .into_inner()
        .account
        .unwrap();
    let bal = cust.balance.unwrap();
    assert_eq!(bal.credits_posted.unwrap().units, 10_000_000);
    assert_eq!(bal.debits_posted.unwrap().units, 6_000_000);

    let err_xfer = client
        .get_transfer(GetTransferRequest { id: 104 })
        .await
        .expect_err("transfer from failed batch must not exist");
    assert_eq!(err_xfer.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn test_concurrent_apply_batch_group_commit() {
    let env = start_test_server(100).await;
    let mut client = LedgerServiceClient::connect(format!("http://{}", env.addr))
        .await
        .expect("connect");

    client
        .create_account(CreateAccountRequest {
            id: 1,
            account_type: AccountType::Asset as i32,
            flags: None,
            scale: 6,
            timestamp: 1,
        })
        .await
        .expect("bank");

    let url = format!("http://{}", env.addr);
    let mut handles = Vec::new();

    for i in 0..10u64 {
        let u = url.clone();
        handles.push(tokio::spawn(async move {
            let mut c = LedgerServiceClient::connect(u).await.unwrap();
            let acc_id = 1000 + i;
            let xfer_id = 2000 + i;
            c.apply_batch(ApplyBatchRequest {
                operations: vec![
                    BatchOperation {
                        operation: Some(ingest::proto::batch_operation::Operation::CreateAccount(
                            CreateAccountRequest {
                                id: acc_id,
                                account_type: AccountType::Liability as i32,
                                flags: None,
                                scale: 6,
                                timestamp: 0,
                            },
                        )),
                    },
                    BatchOperation {
                        operation: Some(ingest::proto::batch_operation::Operation::CreateTransfer(
                            CreateTransferRequest {
                                id: xfer_id,
                                debit_account_id: 1,
                                credit_account_id: acc_id,
                                amount: Some(Amount {
                                    units: 100_000 * (i + 1),
                                    scale: 6,
                                    units_high: 0,
                                }),
                                timestamp: 0,
                            },
                        )),
                    },
                ],
            })
            .await
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let res = h.await.unwrap();
        assert!(res.is_ok(), "concurrent batch {} must succeed", i);
        assert_eq!(res.unwrap().into_inner().applied_count, 2);
    }

    for i in 0..10u64 {
        let acc_id = 1000 + i;
        let acc = client
            .get_account(GetAccountRequest { id: acc_id })
            .await
            .unwrap()
            .into_inner()
            .account
            .unwrap();
        let bal = acc.balance.unwrap();
        assert_eq!(
            bal.credits_posted.unwrap().units,
            100_000 * (i + 1),
            "account {} credit balance mismatch",
            acc_id
        );
    }
}
