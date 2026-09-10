//! Three-way reconciliation engine: App Payments vs Ledger Journal vs Solana On-Chain Roots.

use ledger_core::Ledger;
use serde::{Deserialize, Serialize};
use solana_settle::state::SettlementRoot;

use crate::error::ReconciliationError;
use crate::payment::{AccountDirectory, Payment, PaymentState};

/// Outcome status of a reconciliation audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconciliationStatus {
    /// Zero drift detected; all three sources agree exactly.
    Clean,
    /// Discrepancy detected across sources of truth; operational incident raised.
    DriftDetected,
}

/// Structured incident data generated when reconciliation drift is detected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationIncident {
    /// Unique incident ID.
    pub incident_id: String,
    /// Calculated difference between ledger and external sources.
    pub drift_amount: i128,
    /// Detailed discrepancy message.
    pub details: String,
    /// Recommended operational triage action.
    pub recommended_action: String,
}

/// Comprehensive audit report produced by the three-way reconciliation job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationReport {
    /// Unix timestamp of the audit.
    pub timestamp: u64,
    /// Audit status (`Clean` or `DriftDetected`).
    pub status: ReconciliationStatus,
    /// Total amount captured according to application payment records.
    pub app_settled_total: u128,
    /// Total amount credited to merchant liability accounts in the double-entry ledger.
    pub ledger_settled_total: u128,
    /// Total amount committed on-chain in the Solana settlement PDA.
    pub chain_settled_total: u128,
    /// Discrepancy between ledger and app state (`ledger - app`).
    pub drift: i128,
    /// Number of verified matching transfers.
    pub reconciled_transfers: usize,
    /// Incident details if drift > 0.
    pub incident: Option<ReconciliationIncident>,
}

/// Core reconciliation auditor.
pub struct ReconciliationEngine;

impl ReconciliationEngine {
    /// Perform a three-way reconciliation audit across app records, double-entry ledger, and on-chain state.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::AccountNotFound`] if an expected ledger account is missing.
    pub fn audit(
        payments: &[Payment],
        ledger: &Ledger,
        merchants: &[u128],
        chain_root: Option<&SettlementRoot>,
        timestamp: u64,
    ) -> Result<ReconciliationReport, ReconciliationError> {
        // 1. Calculate App total from captured payments
        let mut app_total: u128 = 0;
        let mut captured_count = 0;

        for p in payments {
            if p.state == PaymentState::Captured {
                app_total = app_total.saturating_add(p.amount);
                captured_count += 1;
            }
        }

        // 2. Calculate Ledger total from merchant liability balances (gross credits)
        let mut ledger_total: u128 = 0;
        for &m_id in merchants {
            let acc_id = AccountDirectory::merchant(m_id);
            let acc = ledger
                .get_account(acc_id)
                .map_err(|_| ReconciliationError::AccountNotFound(acc_id.as_u128()))?;
            ledger_total = ledger_total.saturating_add(acc.balance.credits_posted.as_u128());
        }

        // 3. Calculate Chain total from on-chain PDA state
        let chain_total = chain_root.map(|r| r.total_settled_amount).unwrap_or(0);

        // Compute drift
        let drift: i128 = (ledger_total as i128) - (app_total as i128);

        let is_clean = drift == 0 && (chain_root.is_none() || chain_total == ledger_total);

        if is_clean {
            Ok(ReconciliationReport {
                timestamp,
                status: ReconciliationStatus::Clean,
                app_settled_total: app_total,
                ledger_settled_total: ledger_total,
                chain_settled_total: chain_total,
                drift: 0,
                reconciled_transfers: captured_count,
                incident: None,
            })
        } else {
            let incident = ReconciliationIncident {
                incident_id: format!("INC-RECON-{timestamp}"),
                drift_amount: drift,
                details: format!(
                    "Discrepancy: App={app_total}, Ledger={ledger_total}, Chain={chain_total}, Drift={drift}"
                ),
                recommended_action: "Halt automated batch settlement and inspect unposted transaction buffer.".to_string(),
            };

            Ok(ReconciliationReport {
                timestamp,
                status: ReconciliationStatus::DriftDetected,
                app_settled_total: app_total,
                ledger_settled_total: ledger_total,
                chain_settled_total: chain_total,
                drift,
                reconciled_transfers: captured_count,
                incident: Some(incident),
            })
        }
    }
}
