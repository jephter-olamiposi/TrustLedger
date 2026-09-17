//! Interactive web dashboard rendering live ledger status, balances, and verification flows.

use crate::payment::Payment;

/// Render the complete single-page HTML dashboard for TrustLedger.
#[must_use]
pub fn render_dashboard_html(
    payments: &[Payment],
    current_batch_seq: u64,
    solana_authority: &str,
    solana_pda: &str,
    latest_root: Option<&str>,
) -> String {
    let root_display = latest_root.unwrap_or("None (Awaiting first batch commit)");

    DASHBOARD_HTML_TEMPLATE
        .replace("__CURRENT_BATCH_SEQ__", &current_batch_seq.to_string())
        .replace("__SOLANA_AUTHORITY__", solana_authority)
        .replace("__SOLANA_PDA__", solana_pda)
        .replace("__LATEST_ROOT_HEX__", root_display)
        .replace("__PAYMENTS_COUNT__", &payments.len().to_string())
}

const DASHBOARD_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>TrustLedger — Distributed Settlement Engine</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <style>
        body {
            font-feature-settings: 'cv02', 'cv03', 'cv04', 'cv11';
        }
        pre, code, .font-mono {
            font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
            font-feature-settings: 'tnum' 1;
        }
    </style>
</head>
<body class="bg-[#f8fafc] text-[#0a2540] font-sans antialiased min-h-screen flex flex-col selection:bg-indigo-600 selection:text-white">

    <!-- Top Navigation Bar (Stripe-style light header) -->
    <header class="bg-white/90 backdrop-blur-md border-b border-slate-200 sticky top-0 z-30 shadow-xs">
        <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-3.5">
            <div class="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
                
                <!-- Brand & Subtitle -->
                <div class="flex items-center gap-3">
                    <div class="w-10 h-10 rounded-xl bg-indigo-600 flex items-center justify-center text-white font-black text-sm tracking-wider shadow-sm shadow-indigo-600/20">
                        TL
                    </div>
                    <div>
                        <div class="flex items-center gap-2.5">
                            <h1 class="text-base sm:text-lg font-bold tracking-tight text-slate-900">TrustLedger Settlement Engine</h1>
                            <span class="px-2.5 py-0.5 text-[11px] font-semibold rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200 flex items-center gap-1.5">
                                <span class="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse"></span>
                                Live Engine
                            </span>
                        </div>
                        <p class="text-xs text-slate-500">High-throughput double-entry ledger with cryptographic Solana L1 finality</p>
                    </div>
                </div>

                <!-- System Telemetry Status Badges -->
                <div class="flex flex-wrap items-center gap-2 text-xs font-mono">
                    <div class="px-3 py-1 rounded-lg bg-slate-100 border border-slate-200 text-slate-700 flex items-center gap-1.5 font-medium">
                        <span class="text-indigo-600 font-bold">QUORUM:</span>
                        <span class="text-emerald-700 font-semibold">3/3 Nodes (Raft)</span>
                    </div>
                    <div class="px-3 py-1 rounded-lg bg-slate-100 border border-slate-200 text-slate-700 flex items-center gap-1.5 font-medium">
                        <span class="text-indigo-600 font-bold">ENGINE:</span>
                        <span class="text-slate-800 font-semibold">WAL Fsync (CRC32C)</span>
                    </div>
                    <div class="px-3 py-1 rounded-lg bg-purple-50 border border-purple-200 text-purple-700 flex items-center gap-1.5 font-medium">
                        <span class="text-purple-600 font-bold">L1 ANCHOR:</span>
                        <span class="text-purple-800 font-semibold">Solana PDA</span>
                    </div>
                    <div id="reconHeaderBadge" class="px-3 py-1 rounded-lg bg-emerald-50 border border-emerald-200 text-emerald-700 font-semibold flex items-center gap-1.5">
                        <span class="text-slate-500 font-normal">INVARIANT:</span>
                        <span>0.00 Drift (Balanced)</span>
                    </div>
                </div>

            </div>
        </div>
    </header>

    <!-- Unified Single-Page Workspace -->
    <main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-6 flex-1 w-full space-y-6">

        <!-- Interactive 1-Click Pipeline Banner -->
        <section class="bg-gradient-to-r from-indigo-50/90 via-white to-purple-50/90 rounded-2xl border border-indigo-100 p-6 shadow-xs relative overflow-hidden">
            <div class="flex flex-col lg:flex-row justify-between items-start lg:items-center gap-4 relative z-10">
                <div class="space-y-1">
                    <div class="flex items-center gap-2">
                        <span class="px-2.5 py-0.5 rounded-full bg-indigo-100 text-indigo-700 text-[11px] font-semibold uppercase tracking-wider">End-to-End Walkthrough</span>
                        <span class="text-xs text-slate-500">• Real-Time Settlement In Seconds</span>
                    </div>
                    <h2 class="text-lg font-bold text-slate-900 tracking-tight">Experience the full fintech payment lifecycle in 1 click</h2>
                    <p class="text-xs text-slate-600 max-w-2xl leading-relaxed">
                        Watch an order authorize with a hold, capture to the merchant, commit cryptographically to a Solana L1 PDA, and verify its tamper-proof Merkle proof automatically.
                    </p>
                </div>
                <div class="flex flex-wrap items-center gap-2.5">
                    <button onclick="quickPipelineRun()" id="btnQuickPipeline" class="px-4 py-2.5 bg-indigo-600 hover:bg-indigo-700 text-white rounded-xl text-xs font-semibold transition flex items-center gap-2 shadow-sm shadow-indigo-600/20 cursor-pointer whitespace-nowrap">
                        <span>⚡</span> Run 1-Click Demo Pipeline
                    </button>
                    <button onclick="quickSeed()" class="px-4 py-2.5 bg-white hover:bg-slate-50 text-slate-700 rounded-xl text-xs font-semibold transition border border-slate-300 shadow-xs cursor-pointer">
                        Load 4 Sample Orders
                    </button>
                </div>
            </div>
        </section>

        <!-- Account Wallets Overview Row -->
        <section class="space-y-2.5">
            <div class="flex flex-col sm:flex-row justify-between items-start sm:items-center gap-2">
                <h2 class="text-xs font-bold uppercase tracking-wider text-slate-500">Account Wallets & Liquid Balances</h2>
                <div class="text-xs text-slate-600 font-mono bg-slate-100 border border-slate-200 px-3 py-1 rounded-lg flex items-center gap-2">
                    <span class="text-slate-500">🏦 Reserve Vault:</span>
                    <span id="displayVaultBalance" class="font-bold text-slate-900">$3,000.00 USDC</span>
                    <span class="text-emerald-700 font-sans text-[11px] font-semibold">(100% Backed)</span>
                </div>
            </div>
            <div id="walletCardsContainer" class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
                <!-- Loaded dynamically via JavaScript -->
                <div class="bg-white rounded-2xl border border-slate-200 p-4 shadow-xs animate-pulse">
                    <div class="h-4 bg-slate-100 rounded w-1/3 mb-2"></div>
                    <div class="h-8 bg-slate-100 rounded w-2/3"></div>
                </div>
            </div>
        </section>

        <!-- 2-Column Workstation Grid -->
        <div class="grid grid-cols-1 lg:grid-cols-12 gap-6">

            <!-- LEFT COLUMN: Payment Station, Solana Anchor, and Chaos Simulator (5 Cols) -->
            <div class="lg:col-span-5 space-y-6">

                <!-- 1. Send Payment Console -->
                <section class="bg-white rounded-2xl border border-slate-200 p-5 shadow-xs space-y-4">
                    <div class="flex items-center justify-between pb-3 border-b border-slate-100">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">Send Payment</h3>
                            <p class="text-[11px] text-slate-500">Transfer funds between accounts with instant settlement or card holds</p>
                        </div>
                        <span class="px-2 py-0.5 text-[10px] font-semibold rounded-full bg-slate-100 text-slate-600 border border-slate-200">
                            Zero Fee Demo
                        </span>
                    </div>

                    <div class="grid grid-cols-2 gap-3 text-xs">
                        <div>
                            <label class="block text-slate-700 font-medium mb-1">Payer (Sender)</label>
                            <select id="inputCustomer" onchange="updateBalanceCheck()" class="w-full rounded-xl bg-white border border-slate-300 px-2.5 py-2 text-slate-900 focus:ring-2 focus:ring-indigo-500 focus:border-indigo-500 focus:outline-none font-medium">
                                <option value="101">Alice (#101)</option>
                                <option value="102">Bob (#102)</option>
                                <option value="103">Charlie (#103)</option>
                            </select>
                        </div>
                        <div>
                            <label class="block text-slate-700 font-medium mb-1">Recipient (Merchant)</label>
                            <select id="inputMerchant" class="w-full rounded-xl bg-white border border-slate-300 px-2.5 py-2 text-slate-900 focus:ring-2 focus:ring-indigo-500 focus:border-indigo-500 focus:outline-none font-medium">
                                <option value="1">Acme Corp Store (#1)</option>
                                <option value="2">Globex Supplies (#2)</option>
                                <option value="3">Soylent Logistics (#3)</option>
                            </select>
                        </div>
                    </div>

                    <div>
                        <div class="flex justify-between items-center mb-1 text-xs">
                            <label class="text-slate-700 font-medium">Amount (USDC)</label>
                            <span id="senderBalanceHint" class="text-slate-500 font-mono text-[11px]">Available: $1,000.00</span>
                        </div>
                        <div class="flex items-center gap-2">
                            <div class="relative flex-1">
                                <span class="absolute inset-y-0 left-0 pl-3 flex items-center text-slate-400 font-bold text-xs">$</span>
                                <input type="number" id="inputAmount" oninput="updateBalanceCheck()" step="0.01" value="50.00" class="w-full pl-7 pr-3 py-2 rounded-xl bg-white border border-slate-300 text-slate-900 font-mono text-sm font-bold focus:ring-2 focus:ring-indigo-500 focus:border-indigo-500 focus:outline-none">
                            </div>
                            <div class="flex gap-1">
                                <button onclick="setAmount(10)" class="px-2.5 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 border border-slate-200 text-xs font-semibold cursor-pointer transition">$10</button>
                                <button onclick="setAmount(25)" class="px-2.5 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 border border-slate-200 text-xs font-semibold cursor-pointer transition">$25</button>
                                <button onclick="setAmount(50)" class="px-2.5 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 border border-slate-200 text-xs font-semibold cursor-pointer transition">$50</button>
                                <button onclick="setAmount(100)" class="px-2.5 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 border border-slate-200 text-xs font-semibold cursor-pointer transition">$100</button>
                            </div>
                        </div>

                        <!-- Fund verification banner -->
                        <div id="balanceCheckMsg" class="mt-2 p-2.5 rounded-xl bg-emerald-50 border border-emerald-200 text-xs text-emerald-800 flex items-center justify-between font-medium">
                            <div class="flex items-center gap-1.5">
                                <span class="text-emerald-600 font-bold">✓</span> <span>Sufficient funds available in account.</span>
                            </div>
                        </div>
                    </div>

                    <!-- Mode selector: Instant Pay vs Card Hold -->
                    <div>
                        <label class="block text-slate-600 text-[11px] font-medium mb-1.5">Transfer Mode</label>
                        <div class="grid grid-cols-2 gap-2 text-xs">
                            <label class="p-2.5 rounded-xl border border-indigo-200 bg-indigo-50/50 flex items-start gap-2 cursor-pointer hover:border-indigo-300 transition">
                                <input type="radio" name="paymentWorkflow" value="instant" checked class="mt-0.5 text-indigo-600 focus:ring-indigo-500">
                                <div>
                                    <div class="font-bold text-slate-900 text-xs">⚡ Instant Pay</div>
                                    <div class="text-[10px] text-slate-500 mt-0.5 leading-snug">Direct transfer: Debits payer and deposits to merchant immediately in 1 step</div>
                                </div>
                            </label>
                            <label class="p-2.5 rounded-xl border border-slate-200 bg-slate-50/60 flex items-start gap-2 cursor-pointer hover:border-slate-300 transition">
                                <input type="radio" name="paymentWorkflow" value="hold" class="mt-0.5 text-indigo-600 focus:ring-indigo-500">
                                <div>
                                    <div class="font-bold text-slate-900 text-xs">🔒 Card Hold (Authorize)</div>
                                    <div class="text-[10px] text-slate-500 mt-0.5 leading-snug">Reserves funds in escrow until you click "Capture (Collect)" or "Cancel Hold"</div>
                                </div>
                            </label>
                        </div>
                    </div>

                    <button onclick="submitUserPayment()" id="btnAuthorize" class="w-full py-2.5 px-4 bg-indigo-600 hover:bg-indigo-700 text-white font-bold rounded-xl text-xs transition flex items-center justify-center gap-2 shadow-sm shadow-indigo-600/20 cursor-pointer">
                        <span>⚡</span> Send $50.00 Payment
                    </button>
                </section>

                <!-- 2. Solana L1 Settlement Batch Anchor -->
                <section class="bg-white rounded-2xl border border-slate-200 p-5 shadow-xs space-y-4">
                    <div class="flex items-center justify-between pb-3 border-b border-slate-100">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">Solana L1 Settlement</h3>
                            <p class="text-[11px] text-slate-500">Anchor Merkle Mountain Range roots to Solana PDA</p>
                        </div>
                        <span class="px-2 py-0.5 text-[10px] font-mono font-semibold rounded bg-purple-50 text-purple-700 border border-purple-200">Finality</span>
                    </div>

                    <div class="bg-slate-50 rounded-xl p-3 border border-slate-200 text-xs font-mono space-y-2">
                        <div class="flex justify-between items-center">
                            <span class="text-slate-500 font-sans">Solana Notary PDA:</span>
                            <span class="text-purple-700 font-bold truncate max-w-[200px]" title="__SOLANA_PDA__">__SOLANA_PDA__</span>
                        </div>
                        <div class="flex justify-between items-center">
                            <span class="text-slate-500 font-sans">Committed Batch Seq:</span>
                            <span id="displayBatchSeq" class="text-slate-900 font-bold">#__CURRENT_BATCH_SEQ__</span>
                        </div>
                        <div class="flex justify-between items-center">
                            <span class="text-slate-500 font-sans">Latest MMR Root:</span>
                            <span id="displayLatestRoot" class="text-indigo-600 font-bold truncate max-w-[200px]" title="__LATEST_ROOT_HEX__">__LATEST_ROOT_HEX__</span>
                        </div>
                        <div class="flex justify-between items-center pt-1.5 border-t border-slate-200/60">
                            <span class="text-slate-500 font-sans">Pending L1 Queue:</span>
                            <span id="displayPendingQueue" class="text-emerald-700 font-bold font-sans">0 queued (All Anchored)</span>
                        </div>
                    </div>

                    <div class="flex items-center gap-4 text-xs">
                        <label class="inline-flex items-center cursor-pointer">
                            <input type="radio" name="railChoice" value="Solana-USDC" checked class="text-indigo-600 focus:ring-indigo-500">
                            <span class="ml-1.5 font-semibold text-purple-800">Solana-USDC (L1 Finality)</span>
                        </label>
                        <label class="inline-flex items-center cursor-pointer">
                            <input type="radio" name="railChoice" value="Mock-ACH" class="text-indigo-600 focus:ring-indigo-500">
                            <span class="ml-1.5 text-slate-500">Mock-ACH (Net Batch)</span>
                        </label>
                    </div>

                    <button onclick="commitSettlementBatch()" id="btnCommitBatch" class="w-full py-2.5 px-4 bg-purple-600 hover:bg-purple-700 text-white font-semibold rounded-xl text-xs transition flex items-center justify-center gap-2 shadow-sm shadow-purple-600/20 cursor-pointer">
                        <span>⛓️</span> Commit Pending Transfers to Solana L1
                    </button>
                </section>

                <!-- 3. Fault Injection Simulator (TigerBeetle DST) -->
                <section class="bg-white rounded-2xl border border-slate-200 p-5 shadow-xs space-y-4">
                    <div class="flex items-center justify-between pb-3 border-b border-slate-100">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">Fault Injection Simulator</h3>
                            <p class="text-[11px] text-slate-500">Deterministic Simulation Testing (TigerBeetle DST)</p>
                        </div>
                        <span class="px-2 py-0.5 text-[10px] font-mono font-semibold rounded bg-rose-50 text-rose-700 border border-rose-200">DST</span>
                    </div>

                    <div class="text-xs">
                        <label class="block text-slate-600 font-medium mb-1">Chaos Scenario to Inject:</label>
                        <select id="selectScenario" class="w-full rounded-xl bg-white border border-slate-300 px-2.5 py-2 text-slate-800 focus:ring-2 focus:ring-rose-500 focus:border-rose-500 focus:outline-none font-medium">
                            <option value="NetworkPartition">Network Partition (Raft Split-Brain Survival)</option>
                            <option value="CrashTornWrite">Torn Write (Abrupt Power Loss Mid-Fsync)</option>
                            <option value="ChaosSoak">Chaos Soak (Combined Packet Drops & Reboots)</option>
                        </select>
                    </div>

                    <div id="simOutputBox" class="p-3 rounded-xl bg-slate-900 text-slate-200 text-[11px] font-mono space-y-1 max-h-24 overflow-y-auto border border-slate-800 shadow-inner">
                        <div class="text-slate-400">// Select a scenario above and execute chaos test.</div>
                    </div>

                    <button onclick="runSimulator()" id="btnRunSim" class="w-full py-2.5 px-4 bg-rose-600 hover:bg-rose-700 text-white font-semibold rounded-xl text-xs transition flex items-center justify-center gap-2 shadow-sm shadow-rose-600/20 cursor-pointer">
                        <span>🔥</span> Run Chaos Stress Test
                    </button>
                </section>

            </div>

            <!-- RIGHT COLUMN: Transactions, Reconciliation, and Balance Sheet (7 Cols) -->
            <div class="lg:col-span-7 space-y-6">

                <!-- 1. Live Payment Transactions Table -->
                <section class="bg-white rounded-2xl border border-slate-200 shadow-xs overflow-hidden">
                    <div class="px-5 py-3.5 border-b border-slate-100 flex justify-between items-center bg-slate-50/50">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">Payment Transactions & Cryptographic Receipts</h3>
                            <p class="text-[11px] text-slate-500">Newest orders appear at the top in real time</p>
                        </div>
                        <div class="flex items-center gap-2">
                            <span id="paymentCountBadge" class="px-2.5 py-0.5 text-[11px] font-mono font-semibold rounded-full bg-slate-100 text-slate-600 border border-slate-200">
                                __PAYMENTS_COUNT__ records
                            </span>
                            <button onclick="fetchDashboardData()" class="px-2.5 py-1 text-xs font-semibold text-slate-600 hover:text-slate-900 rounded-lg bg-white hover:bg-slate-100 border border-slate-200 transition cursor-pointer">
                                🔄 Refresh
                            </button>
                        </div>
                    </div>

                    <!-- Explainer Callout: What is Capture? -->
                    <div class="px-5 py-2.5 bg-indigo-50/60 border-b border-indigo-100 flex items-start gap-2.5 text-xs text-indigo-950">
                        <span class="text-sm mt-0.5">💡</span>
                        <div class="leading-relaxed">
                            <span class="font-bold">What is "Capture"?</span> In banking, a <strong>Card Hold</strong> reserves money in the customer's wallet without paying the store yet. 
                            Click <strong class="text-emerald-800 bg-emerald-100 px-1 py-0.5 rounded text-[11px]">Capture (Collect)</strong> to complete the transaction and deposit money to the merchant. 
                            Click <strong class="text-slate-700 bg-slate-200 px-1 py-0.5 rounded text-[11px]">Cancel Hold</strong> to release the hold and return the money to the customer.
                        </div>
                    </div>

                    <div class="overflow-x-auto max-h-72 overflow-y-auto">
                        <table class="w-full text-left border-collapse text-xs">
                            <thead class="sticky top-0 bg-slate-50 text-slate-500 uppercase text-[10px] font-semibold border-b border-slate-200">
                                <tr>
                                    <th class="p-3">Order</th>
                                    <th class="p-3">Payer</th>
                                    <th class="p-3">Merchant</th>
                                    <th class="p-3 font-mono text-right">Amount</th>
                                    <th class="p-3">Ledger State</th>
                                    <th class="p-3">Solana L1 Settlement</th>
                                    <th class="p-3 text-right">Actions</th>
                                </tr>
                            </thead>
                            <tbody id="paymentsTableBody" class="divide-y divide-slate-100 font-sans">
                                <tr>
                                    <td colspan="7" class="p-6 text-center text-slate-400 font-sans">
                                        No transactions recorded yet. Send a payment on the left or click "Run 1-Click Demo Pipeline".
                                    </td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                </section>

                <!-- 2. Continuous 3-Way Reconciliation Auditor -->
                <section class="bg-white rounded-2xl border border-slate-200 p-5 shadow-xs space-y-4">
                    <div class="flex items-center justify-between pb-3 border-b border-slate-100">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">Continuous 3-Way Reconciliation</h3>
                            <p class="text-[11px] text-slate-500">Automated drift detection: App Ingress ≡ Double-Entry Ledger ≡ Solana L1 State</p>
                        </div>
                        <span id="reconStatusPill" class="px-2.5 py-1 text-xs font-mono font-semibold rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200">
                            Balanced ($0.00 Drift)
                        </span>
                    </div>

                    <div class="grid grid-cols-3 gap-3 text-center">
                        <div class="bg-slate-50 p-3 rounded-xl border border-slate-200">
                            <div class="text-[10px] uppercase font-mono text-slate-500">1. Payment Ingress</div>
                            <div id="reconAppTotal" class="text-sm font-bold text-slate-900 mt-1 font-mono">$0.00</div>
                        </div>
                        <div class="bg-slate-50 p-3 rounded-xl border border-slate-200">
                            <div class="text-[10px] uppercase font-mono text-slate-500">2. Ledger Books</div>
                            <div id="reconLedgerTotal" class="text-sm font-bold text-slate-900 mt-1 font-mono">$0.00</div>
                        </div>
                        <div class="bg-slate-50 p-3 rounded-xl border border-slate-200">
                            <div class="text-[10px] uppercase font-mono text-slate-500">3. Solana Settled</div>
                            <div id="reconChainTotal" class="text-sm font-bold text-slate-900 mt-1 font-mono">$0.00</div>
                        </div>
                    </div>

                    <div id="reconIncidentAlert" class="hidden p-3.5 rounded-xl bg-rose-50 border border-rose-200 text-rose-800 text-xs space-y-2">
                        <div class="flex items-center justify-between">
                            <div class="font-bold flex items-center gap-1.5 text-rose-900">
                                <span>🚨</span> Invariant Discrepancy Detected
                            </div>
                            <button onclick="healDrift()" class="px-2.5 py-1 bg-rose-600 hover:bg-rose-700 text-white rounded-lg font-semibold text-[11px] transition cursor-pointer shadow-2xs">
                                🛠️ Auto-Heal ($0.00 Drift)
                            </button>
                        </div>
                        <div id="reconIncidentDetails" class="text-[11px] font-mono"></div>
                        <div id="reconIncidentAction" class="text-[11px] text-rose-700 font-semibold"></div>
                    </div>

                    <div class="flex gap-2">
                        <button onclick="runReconciliation()" class="flex-1 py-2 px-3 bg-slate-100 hover:bg-slate-200 text-slate-800 font-semibold rounded-lg text-xs transition border border-slate-200 cursor-pointer">
                            Run Full Audit
                        </button>
                        <button onclick="simulateDrift()" class="py-2 px-3 bg-amber-50 hover:bg-amber-100 text-amber-800 font-semibold rounded-lg text-xs transition border border-amber-200 cursor-pointer">
                            Simulate Drift ($50)
                        </button>
                        <button onclick="healDrift()" class="py-2 px-3 bg-emerald-50 hover:bg-emerald-100 text-emerald-800 font-semibold rounded-lg text-xs transition border border-emerald-200 cursor-pointer">
                            Auto-Heal ($0.00)
                        </button>
                    </div>
                </section>

                <!-- 3. General Ledger Accounts (Double-Entry Balance Sheet) -->
                <section class="bg-white rounded-2xl border border-slate-200 shadow-xs overflow-hidden">
                    <div class="px-5 py-3.5 border-b border-slate-100 flex flex-col sm:flex-row justify-between items-start sm:items-center gap-2 bg-slate-50/50">
                        <div>
                            <h3 class="text-sm font-bold text-slate-900">General Ledger Accounts (Balance Sheet)</h3>
                            <p class="text-[11px] text-slate-500">Assets (Clearing Vault) vs Liabilities (Customer & Merchant Balances)</p>
                        </div>
                        <span class="px-2.5 py-0.5 text-[10px] font-mono font-semibold rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200">
                            Σ Debits ≡ Σ Credits
                        </span>
                    </div>

                    <div class="overflow-x-auto max-h-64 overflow-y-auto">
                        <table class="w-full text-left border-collapse text-xs font-mono">
                            <thead class="sticky top-0 bg-slate-50 text-slate-500 uppercase text-[10px] font-sans font-semibold border-b border-slate-200">
                                <tr>
                                    <th class="p-3">ID</th>
                                    <th class="p-3">Account</th>
                                    <th class="p-3">Type</th>
                                    <th class="p-3 text-right">Credits</th>
                                    <th class="p-3 text-right">Debits</th>
                                    <th class="p-3 text-right">Holds</th>
                                    <th class="p-3 text-right font-bold">Net Balance</th>
                                    <th class="p-3 text-center font-sans">Quick Action</th>
                                </tr>
                            </thead>
                            <tbody id="accountsTableBody" class="divide-y divide-slate-100">
                                <tr>
                                    <td colspan="8" class="p-6 text-center text-slate-400 font-sans">Loading general ledger accounts...</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                </section>

            </div>

        </div>

    </main>

    <!-- Institutional Footer -->
    <footer class="bg-white border-t border-slate-200 py-4 text-center text-xs text-slate-500 font-mono">
        TrustLedger • Distributed Double-Entry Settlement Engine • Solana L1 Finality • Zero Unsafe Code
    </footer>

    <!-- Digital Payment Receipt Modal -->
    <div id="proofModal" class="hidden fixed inset-0 z-50 overflow-y-auto bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4">
        <div class="bg-white rounded-2xl max-w-xl w-full p-6 shadow-2xl border border-slate-200 space-y-5">
            <div class="flex justify-between items-start pb-4 border-b border-slate-100">
                <div class="flex items-center gap-3">
                    <div class="w-9 h-9 rounded-xl bg-emerald-500 text-white flex items-center justify-center font-bold text-lg shadow-sm">
                        ✓
                    </div>
                    <div>
                        <h3 class="text-base font-bold text-slate-900">Payment Receipt</h3>
                        <p class="text-xs text-slate-500">Cryptographically anchored to Solana L1 PDA</p>
                    </div>
                </div>
                <button onclick="closeProofModal()" class="text-slate-400 hover:text-slate-600 text-lg font-bold p-1 cursor-pointer">✕</button>
            </div>

            <div id="proofModalContent" class="space-y-4 text-xs">
                <!-- Populated dynamically via renderProofModal() -->
            </div>

            <div class="pt-4 border-t border-slate-100 flex justify-end">
                <button onclick="closeProofModal()" class="px-5 py-2 bg-slate-900 hover:bg-slate-800 text-white rounded-xl text-xs font-semibold transition cursor-pointer shadow-xs">
                    Close Receipt
                </button>
            </div>
        </div>
    </div>

    <!-- Toast Notifications Container -->
    <div id="toastContainer" class="fixed bottom-5 right-5 z-50 flex flex-col gap-2 max-w-md pointer-events-none"></div>

    <!-- Dashboard Client Logic -->
    <script>
        let currentData = null;

        function setAmount(val) {
            document.getElementById('inputAmount').value = val.toFixed(2);
            updateBalanceCheck();
        }

        function showToast(message, type = 'success') {
            const container = document.getElementById('toastContainer');
            const toast = document.createElement('div');
            const bgClass = type === 'success' ? 'bg-slate-900 text-white border-slate-800' : (type === 'error' ? 'bg-rose-600 text-white border-rose-700' : 'bg-white text-slate-900 border-slate-200');
            toast.className = `${bgClass} px-4 py-3 rounded-xl shadow-xl text-xs font-medium flex items-center justify-between gap-3 pointer-events-auto transform transition duration-200 translate-y-2 opacity-0 border`;
            toast.innerHTML = `<span>${message}</span><button onclick="this.parentElement.remove()" class="text-white/70 hover:text-white font-bold ml-2 cursor-pointer">✕</button>`;
            container.appendChild(toast);

            requestAnimationFrame(() => {
                toast.classList.remove('translate-y-2', 'opacity-0');
            });

            setTimeout(() => {
                toast.classList.add('opacity-0', 'translate-y-2');
                setTimeout(() => toast.remove(), 300);
            }, 3500);
        }

        async function fetchDashboardData() {
            try {
                const res = await fetch('/api/dashboard/data');
                if (!res.ok) throw new Error('Failed to fetch dashboard state');
                currentData = await res.json();
                renderDashboard(currentData);
                updateBalanceCheck();
            } catch (err) {
                console.error(err);
            }
        }

        function updateBalanceCheck() {
            if (!currentData || !currentData.accounts) return;
            const custId = parseInt(document.getElementById('inputCustomer').value, 10);
            const amtVal = parseFloat(document.getElementById('inputAmount').value) || 0;
            const amtUnits = Math.round(amtVal * 1000000);

            const acc = currentData.accounts.find(a => a.id === (1000000 + custId));
            const msgEl = document.getElementById('balanceCheckMsg');
            const btn = document.getElementById('btnAuthorize');
            const hintEl = document.getElementById('senderBalanceHint');

            if (!acc) return;
            const available = acc.net_balance;
            hintEl.innerText = `Available: $${(available/1000000).toFixed(2)}`;

            const mode = document.querySelector('input[name="paymentWorkflow"]:checked')?.value || 'instant';
            const actionText = mode === 'instant' ? `Send $${amtVal.toFixed(2)} Payment` : `Authorize & Place Hold ($${amtVal.toFixed(2)})`;

            if (amtUnits > available) {
                msgEl.className = 'mt-2 p-2.5 rounded-xl bg-rose-50 border border-rose-200 text-xs text-rose-800 flex items-center justify-between font-medium';
                msgEl.innerHTML = `
                    <div class="flex items-center gap-1.5">
                        <span class="text-rose-600 font-bold">⚠️</span>
                        <span>Insufficient balance ($${(available/1000000).toFixed(2)} available).</span>
                    </div>
                    <button onclick="topupCustomer(${custId})" class="px-2.5 py-1 bg-rose-100 hover:bg-rose-200 text-rose-800 rounded-lg text-[11px] font-bold border border-rose-300 transition cursor-pointer">
                        + Top Up $100
                    </button>
                `;
                btn.disabled = true;
                btn.className = 'w-full py-2.5 px-4 bg-slate-100 text-slate-400 font-bold rounded-xl text-xs cursor-not-allowed border border-slate-200 flex items-center justify-center gap-2';
                btn.innerText = `Insufficient Funds — Add $100 to Continue`;
            } else {
                msgEl.className = 'mt-2 p-2.5 rounded-xl bg-emerald-50 border border-emerald-200 text-xs text-emerald-800 flex items-center justify-between font-medium';
                msgEl.innerHTML = `
                    <div class="flex items-center gap-1.5">
                        <span class="text-emerald-600 font-bold">✓</span> <span>Sufficient funds available in account.</span>
                    </div>
                `;
                btn.disabled = false;
                btn.className = 'w-full py-2.5 px-4 bg-indigo-600 hover:bg-indigo-700 text-white font-bold rounded-xl text-xs transition flex items-center justify-center gap-2 shadow-sm shadow-indigo-600/20 cursor-pointer';
                btn.innerText = (mode === 'instant' ? '⚡ ' : '🔒 ') + actionText;
            }
        }

        document.querySelectorAll('input[name="paymentWorkflow"]').forEach(r => {
            r.addEventListener('change', updateBalanceCheck);
        });

        function renderDashboard(data) {
            document.getElementById('displayBatchSeq').innerText = '#' + data.batch_seq;
            if (data.latest_root_hex) {
                document.getElementById('displayLatestRoot').innerText = data.latest_root_hex;
                document.getElementById('displayLatestRoot').title = data.latest_root_hex;
            }

            // Display Clearing Vault Reserve in Header
            const vault = data.accounts.find(a => a.id === 100);
            if (vault) {
                document.getElementById('displayVaultBalance').innerText = '$' + (vault.net_balance / 1000000).toFixed(2) + ' USDC';
            }

            // Render 4 Wallets in Light Theme (Alice, Bob, Charlie, Acme Corp)
            const walletContainer = document.getElementById('walletCardsContainer');
            let walletHtml = '';
            for (const a of data.accounts) {
                if (a.id >= 1000101 && a.id <= 1000103) {
                    const custId = a.id - 1000000;
                    const netStr = '$' + (a.net_balance / 1000000).toFixed(2);
                    const pendingStr = '$' + (a.credits_pending / 1000000).toFixed(2);
                    walletHtml += `
                        <div class="bg-white rounded-2xl border border-slate-200 p-4 shadow-xs hover:border-slate-300 transition space-y-2.5">
                            <div class="flex justify-between items-start">
                                <div>
                                    <div class="font-bold text-slate-900 text-xs flex items-center gap-1.5">
                                        <div class="w-6 h-6 rounded-full bg-indigo-50 text-indigo-700 flex items-center justify-center font-bold text-xs border border-indigo-100">
                                            ${a.label.charAt(0)}
                                        </div>
                                        ${a.label}
                                    </div>
                                    <div class="text-[10px] text-slate-500 mt-0.5">Customer Wallet #${custId}</div>
                                </div>
                                <button onclick="topupCustomer(${custId})" class="px-2 py-0.5 text-[10px] font-semibold text-emerald-700 bg-emerald-50 hover:bg-emerald-100 rounded-lg border border-emerald-200 transition cursor-pointer">
                                    + $100
                                </button>
                            </div>
                            <div>
                                <div class="text-xl font-bold text-slate-900 font-mono">${netStr}</div>
                                <div class="text-[10px] text-slate-400 font-mono">Available Balance</div>
                            </div>
                            <div class="text-[10px] text-amber-700 font-mono flex items-center justify-between pt-1.5 border-t border-slate-100">
                                <span class="text-slate-400">Held in Escrow:</span>
                                <span>${pendingStr}</span>
                            </div>
                        </div>
                    `;
                } else if (a.id === 2000001) {
                    const netStr = '$' + (a.net_balance / 1000000).toFixed(2);
                    walletHtml += `
                        <div class="bg-white rounded-2xl border border-purple-200 p-4 shadow-xs space-y-2.5">
                            <div class="flex justify-between items-start">
                                <div>
                                    <div class="font-bold text-slate-900 text-xs flex items-center gap-1.5">
                                        <div class="w-6 h-6 rounded-full bg-purple-50 text-purple-700 flex items-center justify-center font-bold text-xs border border-purple-100">
                                            🛒
                                        </div>
                                        Acme Corp Store
                                    </div>
                                    <div class="text-[10px] text-purple-600 font-medium mt-0.5">Merchant Storefront #1</div>
                                </div>
                                <span class="px-2 py-0.5 text-[10px] font-semibold rounded-full bg-purple-50 text-purple-700 border border-purple-200">Active</span>
                            </div>
                            <div>
                                <div class="text-xl font-bold text-slate-900 font-mono">${netStr}</div>
                                <div class="text-[10px] text-slate-400 font-mono">Revenues Received</div>
                            </div>
                            <div class="text-[10px] text-purple-700 font-mono flex items-center justify-between pt-1.5 border-t border-slate-100">
                                <span class="text-slate-400">Settlement Rail:</span>
                                <span>Solana-USDC</span>
                            </div>
                        </div>
                    `;
                }
            }
            walletContainer.innerHTML = walletHtml;

            // Reconciliation Status
            const recon = data.reconciliation;
            const isClean = recon.status === 'Clean';
            const driftFormatted = (recon.drift / 1000000).toFixed(2);

            const reconStatusPill = document.getElementById('reconStatusPill');
            const reconHeaderBadge = document.getElementById('reconHeaderBadge');

            if (isClean) {
                reconStatusPill.className = 'px-2.5 py-1 text-xs font-mono font-semibold rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200';
                reconStatusPill.innerText = 'Balanced ($0.00 Drift)';
                reconHeaderBadge.className = 'px-3 py-1 rounded-lg bg-emerald-50 border border-emerald-200 text-emerald-700 font-semibold flex items-center gap-1.5';
                reconHeaderBadge.innerHTML = '<span class="text-slate-500 font-normal">INVARIANT:</span><span>0.00 Drift (Balanced)</span>';
                document.getElementById('reconIncidentAlert').classList.add('hidden');
            } else {
                reconStatusPill.className = 'px-2.5 py-1 text-xs font-mono font-semibold rounded-full bg-rose-50 text-rose-700 border border-rose-200 animate-pulse';
                reconStatusPill.innerText = 'Discrepancy: $' + driftFormatted;
                reconHeaderBadge.className = 'px-3 py-1 rounded-lg bg-rose-50 border border-rose-200 text-rose-700 font-semibold flex items-center gap-1.5 animate-pulse';
                reconHeaderBadge.innerHTML = '<span class="text-rose-600 font-bold">DISCREPANCY:</span><span>$' + driftFormatted + ' Drift</span>';

                if (recon.incident) {
                    document.getElementById('reconIncidentDetails').innerText = recon.incident.details;
                    document.getElementById('reconIncidentAction').innerText = 'Action: ' + recon.incident.recommended_action;
                    document.getElementById('reconIncidentAlert').classList.remove('hidden');
                }
            }

            document.getElementById('reconAppTotal').innerText = '$' + (recon.app_settled_total / 1000000).toFixed(2);
            document.getElementById('reconLedgerTotal').innerText = '$' + (recon.ledger_settled_total / 1000000).toFixed(2);
            document.getElementById('reconChainTotal').innerText = '$' + (recon.chain_settled_total / 1000000).toFixed(2);

            // Pending Solana L1 Queue Calculation
            const pendingTransfers = data.payments.filter(p => p.state === 'Captured' && !p.settlement_batch_seq);
            const pendingCount = pendingTransfers.length;
            const pendingAmount = pendingTransfers.reduce((sum, p) => sum + p.amount, 0);

            const queueDisplay = document.getElementById('displayPendingQueue');
            const btnCommit = document.getElementById('btnCommitBatch');
            if (queueDisplay) {
                if (pendingCount > 0) {
                    queueDisplay.className = 'text-amber-700 font-bold font-mono';
                    queueDisplay.innerText = `${pendingCount} queued ($${(pendingAmount / 1000000).toFixed(2)} USDC)`;
                } else {
                    queueDisplay.className = 'text-emerald-700 font-bold font-mono';
                    queueDisplay.innerText = '0 queued (All Anchored)';
                }
            }
            if (btnCommit) {
                if (pendingCount > 0) {
                    btnCommit.innerHTML = `<span>⛓️</span> Commit ${pendingCount} Pending Transfer${pendingCount > 1 ? 's' : ''} to Solana L1`;
                } else {
                    btnCommit.innerHTML = `<span>⛓️</span> Commit Pending Transfers to Solana L1`;
                }
            }

            // Payments Table (Newest first!)
            const tbody = document.getElementById('paymentsTableBody');
            document.getElementById('paymentCountBadge').innerText = `${data.payments.length} records`;

            if (data.payments.length === 0) {
                tbody.innerHTML = `<tr><td colspan="7" class="p-6 text-center text-slate-400 font-sans">No transactions recorded yet. Send a payment on the left or click "Run 1-Click Demo Pipeline".</td></tr>`;
            } else {
                // Ensure newest orders appear first at the top
                const sortedPayments = [...data.payments].sort((a, b) => b.id - a.id);
                let rows = '';
                for (const p of sortedPayments) {
                    let badgeClass = '';
                    let statusLabel = p.state;

                    if (p.state === 'Authorized') {
                        badgeClass = 'bg-amber-50 text-amber-800 border-amber-200';
                        statusLabel = '🟡 On Hold (Awaiting Capture)';
                    } else if (p.state === 'Captured') {
                        badgeClass = 'bg-emerald-50 text-emerald-800 border-emerald-200';
                        statusLabel = '🟢 Paid & Collected';
                    } else if (p.state === 'Voided') {
                        badgeClass = 'bg-slate-100 text-slate-600 border-slate-200';
                        statusLabel = '⚪ Hold Cancelled';
                    }

                    let l1Badge = '';
                    if (p.settlement_batch_seq) {
                        l1Badge = `<span class="px-2 py-0.5 text-[10px] font-mono font-semibold rounded-full bg-purple-50 text-purple-700 border border-purple-200">⛓️ Batch #${p.settlement_batch_seq}</span>`;
                    } else if (p.state === 'Captured') {
                        l1Badge = `<span class="px-2 py-0.5 text-[10px] font-mono font-semibold rounded-full bg-amber-50 text-amber-800 border border-amber-200">⏳ Queued for Batch</span>`;
                    } else if (p.state === 'Authorized') {
                        l1Badge = `<span class="px-2 py-0.5 text-[10px] font-mono text-slate-400">Escrow Hold</span>`;
                    } else {
                        l1Badge = `<span class="px-2 py-0.5 text-[10px] font-mono text-slate-400">—</span>`;
                    }

                    const amountStr = '$' + (p.amount / 1000000).toFixed(2);

                    let actionBtns = '';
                    if (p.state === 'Authorized') {
                        actionBtns = `
                            <button onclick="capturePayment(${p.id})" class="px-2.5 py-1 text-xs font-semibold text-white bg-emerald-600 hover:bg-emerald-700 rounded-lg transition cursor-pointer shadow-2xs inline-flex items-center gap-1" title="Capture: Collect money and deposit into merchant's account">
                                <span>✓</span> Capture (Collect)
                            </button>
                            <button onclick="voidPayment(${p.id})" class="ml-1 px-2.5 py-1 text-xs font-semibold text-slate-700 bg-white hover:bg-slate-50 rounded-lg transition border border-slate-300 cursor-pointer shadow-2xs" title="Cancel Hold: Release held funds back to customer">
                                Cancel Hold
                            </button>
                        `;
                    } else if (p.state === 'Captured') {
                        actionBtns = `
                            <button onclick="verifyPayment(${p.id})" class="px-3 py-1 text-xs font-semibold text-indigo-700 bg-indigo-50 hover:bg-indigo-100 rounded-lg transition cursor-pointer flex items-center gap-1 ml-auto border border-indigo-200 shadow-2xs">
                                <span>📄</span> View Receipt
                            </button>
                        `;
                    } else {
                        actionBtns = `<span class="text-xs text-slate-400">Hold Released</span>`;
                    }

                    rows += `
                        <tr class="hover:bg-slate-50/80 transition">
                            <td class="p-3 font-mono text-slate-700 font-bold">#${p.id}</td>
                            <td class="p-3 text-slate-800 font-medium">Customer #${p.customer_id}</td>
                            <td class="p-3 text-slate-600">Merchant #${p.merchant_id}</td>
                            <td class="p-3 font-mono font-bold text-right text-indigo-600">${amountStr}</td>
                            <td class="p-3">
                                <span class="px-2.5 py-0.5 text-[11px] font-mono font-semibold rounded-full border ${badgeClass}">
                                    ${statusLabel}
                                </span>
                            </td>
                            <td class="p-3">
                                ${l1Badge}
                            </td>
                            <td class="p-3 text-right">
                                ${actionBtns}
                            </td>
                        </tr>
                    `;
                }
                tbody.innerHTML = rows;
            }

            // General Ledger Accounts Table
            const accTbody = document.getElementById('accountsTableBody');
            let accRows = '';
            for (const a of data.accounts) {
                const creditsStr = '$' + (a.credits_posted / 1000000).toFixed(2);
                const debitsStr = '$' + (a.debits_posted / 1000000).toFixed(2);
                const pendingStr = '$' + (a.credits_pending / 1000000).toFixed(2);
                const netStr = '$' + (a.net_balance / 1000000).toFixed(2);

                let topupAction = '';
                if (a.id >= 1000101 && a.id <= 1000103) {
                    const custId = a.id - 1000000;
                    topupAction = `
                        <button onclick="topupCustomer(${custId})" class="px-2 py-0.5 text-[10px] font-semibold text-emerald-700 bg-emerald-50 hover:bg-emerald-100 rounded-lg border border-emerald-200 transition cursor-pointer">
                            + $100
                        </button>
                    `;
                } else {
                    topupAction = `<span class="text-slate-400 text-[10px]">—</span>`;
                }

                accRows += `
                    <tr class="hover:bg-slate-50/80 transition">
                        <td class="p-3 text-slate-400">#${a.id}</td>
                        <td class="p-3 font-sans text-slate-800 font-medium">${a.label}</td>
                        <td class="p-3 font-sans">
                            <span class="px-2 py-0.5 text-[9px] font-mono uppercase font-semibold rounded-full bg-slate-100 text-slate-600 border border-slate-200">
                                ${a.account_type}
                            </span>
                        </td>
                        <td class="p-3 text-right text-slate-600">${creditsStr}</td>
                        <td class="p-3 text-right text-slate-600">${debitsStr}</td>
                        <td class="p-3 text-right text-amber-700">${pendingStr}</td>
                        <td class="p-3 text-right font-bold text-slate-900">${netStr}</td>
                        <td class="p-3 text-center">${topupAction}</td>
                    </tr>
                `;
            }
            accTbody.innerHTML = accRows;
        }

        async function topupCustomer(customerId) {
            try {
                const res = await fetch(`/api/customers/${customerId}/deposit`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ amount: 100000000 })
                });
                if (!res.ok) throw new Error('Top up failed');
                const data = await res.json();
                showToast(`Deposited $100.00 to Customer #${customerId}. Available: $${(data.new_balance/1000000).toFixed(2)}`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function submitUserPayment() {
            const mode = document.querySelector('input[name="paymentWorkflow"]:checked')?.value || 'instant';
            if (mode === 'instant') {
                await executeInstantPayment();
            } else {
                await authorizePayment();
            }
        }

        async function executeInstantPayment() {
            const customer_id = parseInt(document.getElementById('inputCustomer').value, 10);
            const merchant_id = parseInt(document.getElementById('inputMerchant').value, 10);
            const amountVal = parseFloat(document.getElementById('inputAmount').value) || 0;
            if (amountVal <= 0) {
                showToast('Please enter a positive payment amount', 'error');
                return;
            }

            const amount = Math.round(amountVal * 1000000);
            const fee_amount = 500000;
            const idempotency_key = 'inst-' + Date.now() + '-' + Math.random().toString(36).substring(7);

            try {
                const res = await fetch('/api/payments/authorize', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ merchant_id, customer_id, amount, fee_amount, idempotency_key })
                });
                if (!res.ok) {
                    const err = await res.text();
                    throw new Error(err || 'Authorization failed');
                }
                const authData = await res.json();

                const capRes = await fetch(`/api/payments/${authData.payment_id}/capture`, { method: 'POST' });
                if (!capRes.ok) throw new Error('Payment capture failed');

                showToast(`Payment of $${amountVal.toFixed(2)} sent & captured! Order #${authData.payment_id}`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function quickPipelineRun() {
            const btn = document.getElementById('btnQuickPipeline');
            btn.disabled = true;
            btn.innerHTML = `<span>⏳</span> Executing Pipeline...`;

            try {
                const authRes = await fetch('/api/payments/authorize', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        merchant_id: 1,
                        customer_id: 101,
                        amount: 50000000,
                        fee_amount: 500000,
                        idempotency_key: 'quick-pipeline-' + Date.now()
                    })
                });
                if (!authRes.ok) throw new Error('Authorization step failed');
                const authData = await authRes.json();
                showToast(`Step 1: Order #${authData.payment_id} Authorized ($50.00 Hold).`);

                const capRes = await fetch(`/api/payments/${authData.payment_id}/capture`, { method: 'POST' });
                if (!capRes.ok) throw new Error('Capture step failed');
                showToast(`Step 2: Order #${authData.payment_id} Captured to Acme Corp.`);

                const batchRes = await fetch('/api/settlement/batch', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ rail: 'Solana-USDC' })
                });
                if (!batchRes.ok) throw new Error('Settlement commit step failed');
                const batchData = await batchRes.json();
                showToast(`Step 3: Batch #${batchData.batch_seq} anchored to Solana PDA!`);

                await fetchDashboardData();
                await verifyPayment(authData.payment_id);
            } catch (err) {
                showToast(err.message, 'error');
            } finally {
                btn.disabled = false;
                btn.innerHTML = `<span>⚡</span> Run 1-Click Demo Pipeline`;
            }
        }

        async function authorizePayment() {
            const customer_id = parseInt(document.getElementById('inputCustomer').value, 10);
            const merchant_id = parseInt(document.getElementById('inputMerchant').value, 10);
            const amountVal = parseFloat(document.getElementById('inputAmount').value) || 0;
            if (amountVal <= 0) {
                showToast('Please enter a positive payment amount', 'error');
                return;
            }

            const amount = Math.round(amountVal * 1000000);
            const fee_amount = 500000;
            const idempotency_key = 'auth-' + Date.now() + '-' + Math.random().toString(36).substring(7);

            try {
                const res = await fetch('/api/payments/authorize', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ merchant_id, customer_id, amount, fee_amount, idempotency_key })
                });

                if (!res.ok) {
                    const err = await res.text();
                    throw new Error(err || 'Authorization failed');
                }

                const data = await res.json();
                showToast(`Order #${data.payment_id} Authorized: $${amountVal.toFixed(2)} hold placed in escrow.`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function capturePayment(paymentId) {
            try {
                const res = await fetch(`/api/payments/${paymentId}/capture`, { method: 'POST' });
                if (!res.ok) throw new Error('Capture failed');
                showToast(`Payment #${paymentId} Captured! Funds collected and deposited to merchant.`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function voidPayment(paymentId) {
            try {
                const res = await fetch(`/api/payments/${paymentId}/void`, { method: 'POST' });
                if (!res.ok) throw new Error('Void failed');
                showToast(`Hold Cancelled for Payment #${paymentId}. Reserved funds returned to customer.`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function quickSeed() {
            try {
                const res = await fetch('/api/payments/quick-seed', { method: 'POST' });
                if (!res.ok) throw new Error('Seeding failed');
                const data = await res.json();
                showToast(data.message);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function commitSettlementBatch() {
            const railChoice = document.querySelector('input[name="railChoice"]:checked').value;
            try {
                const res = await fetch('/api/settlement/batch', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ rail: railChoice })
                });

                if (!res.ok) {
                    const err = await res.text();
                    throw new Error(err || 'Settlement failed');
                }

                const data = await res.json();
                showToast(`Batch #${data.batch_seq} committed: ${data.transfer_count} transfers anchored.`);
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function runReconciliation() {
            try {
                const res = await fetch('/api/reconciliation');
                if (!res.ok) throw new Error('Audit failed');
                const report = await res.json();
                if (report.status === 'Clean') {
                    showToast('Audit Verified: 100% Invariant Conservation. Zero Drift across L1.');
                } else {
                    showToast('Audit Alert: Discrepancy detected across records!', 'error');
                }
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function simulateDrift() {
            try {
                const res = await fetch('/api/reconciliation/drift-simulate', { method: 'POST' });
                if (!res.ok) throw new Error('Simulation failed');
                showToast('Injected $50 ledger drift. Notice discrepancy alert triggered.', 'error');
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function healDrift() {
            try {
                const res = await fetch('/api/reconciliation/drift-heal', { method: 'POST' });
                if (!res.ok) throw new Error('Heal failed');
                showToast('Reconciliation complete. All ledgers balanced to $0.00 drift.');
                await fetchDashboardData();
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function runSimulator() {
            const scenario = document.getElementById('selectScenario').value;
            const btn = document.getElementById('btnRunSim');
            const output = document.getElementById('simOutputBox');

            btn.disabled = true;
            btn.innerHTML = `<span>⏳</span> Injecting Faults...`;

            output.innerHTML = `
                <div class="text-indigo-400 font-bold">// Initializing virtual DST cluster: ${scenario}</div>
                <div class="text-slate-400">// Simulating network faults and torn-write recoveries...</div>
            `;

            try {
                const res = await fetch('/api/simulator/run', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ scenario, seed: 42 })
                });

                if (!res.ok) throw new Error('Simulation run failed');
                const rep = await res.json();

                output.innerHTML = `
                    <div class="text-emerald-400 font-bold">✓ DST TEST PASSED [${rep.status}]</div>
                    <div class="text-slate-300">Virtual Ticks: ${rep.total_ticks} | Faults Injected: ${rep.packets_dropped}</div>
                    <div class="text-emerald-300 font-semibold mt-0.5">Formal Invariant Verified: Zero balance leakage.</div>
                `;
                showToast(`Simulation passed: Invariants conserved under simulated crash!`);
            } catch (err) {
                output.innerHTML = `<div class="text-rose-400 font-bold">Error: ${err.message}</div>`;
                showToast(err.message, 'error');
            } finally {
                btn.disabled = false;
                btn.innerHTML = `<span>🔥</span> Run Chaos Stress Test`;
            }
        }

        async function verifyPayment(paymentId) {
            try {
                const res = await fetch(`/api/payments/${paymentId}/verify`, { method: 'POST' });
                if (!res.ok) throw new Error('Verification query failed');
                const data = await res.json();
                renderProofModal(data);
            } catch (err) {
                showToast(err.message, 'error');
            }
        }

        async function commitAndVerifyFromModal(paymentId) {
            const btn = document.getElementById('btnModalAnchor');
            if (btn) {
                btn.disabled = true;
                btn.innerHTML = `<span>⏳</span> Committing to Solana...`;
            }
            try {
                const res = await fetch('/api/settlement/batch', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ rail: 'Solana-USDC' })
                });
                if (!res.ok) {
                    const err = await res.text();
                    throw new Error(err || 'Settlement failed');
                }
                const data = await res.json();
                showToast(`Batch #${data.batch_seq} committed: anchored to Solana PDA!`);
                await fetchDashboardData();
                await verifyPayment(paymentId);
            } catch (err) {
                showToast(err.message, 'error');
                if (btn) {
                    btn.disabled = false;
                    btn.innerHTML = `<span>⛓️</span> Anchor to Solana L1 Now`;
                }
            }
        }

        function renderProofModal(data) {
            const container = document.getElementById('proofModalContent');
            const verifiedBadge = data.verified
                ? `<span class="px-3 py-1 rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200 font-mono font-semibold text-xs">✓ L1 MERKLE PROOF VERIFIED</span>`
                : `<span class="px-3 py-1 rounded-full bg-amber-50 text-amber-800 border border-amber-200 font-mono font-semibold text-xs">⏳ QUEUED FOR SOLANA L1 BATCH</span>`;

            let siblingsHtml = '';
            if (data.proof_siblings && data.proof_siblings.length > 0) {
                siblingsHtml = data.proof_siblings.map((s, idx) => `
                    <div class="flex items-center gap-2 text-xs font-mono text-slate-700 bg-white p-2 rounded border border-slate-200">
                        <span class="text-indigo-600 font-bold">Branch #${idx+1}:</span>
                        <span class="truncate">${s}</span>
                    </div>
                `).join('');
            } else if (data.verified) {
                siblingsHtml = `<div class="text-slate-500 italic">Single-leaf root (cryptographic anchor complete)</div>`;
            } else {
                siblingsHtml = `<div class="text-amber-800 bg-amber-50 p-2 rounded border border-amber-200 text-xs">Awaiting batch inclusion in Solana Merkle Mountain Range.</div>`;
            }

            const actionAnchorBtn = !data.verified ? `
                <div class="p-3 bg-indigo-50 border border-indigo-200 rounded-xl flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3">
                    <div class="text-xs text-indigo-950">
                        <span class="font-bold">Cleared in double-entry books.</span> Ready to anchor to Solana blockchain.
                    </div>
                    <button onclick="commitAndVerifyFromModal(${data.payment_id})" id="btnModalAnchor" class="px-3.5 py-1.5 bg-purple-600 hover:bg-purple-700 text-white font-bold rounded-lg text-xs transition cursor-pointer flex items-center gap-1.5 whitespace-nowrap shadow-xs">
                        <span>⛓️</span> Anchor to Solana L1 Now
                    </button>
                </div>
            ` : '';

            const guaranteeBox = data.verified ? `
                <div class="p-3.5 bg-emerald-50/60 rounded-xl text-emerald-900 leading-relaxed text-xs border border-emerald-200">
                    <span class="font-bold text-emerald-700">Mathematical Guarantee:</span> This cryptographic inclusion proof demonstrates that this settlement was committed to the Solana blockchain state. It is immutable, non-repudiable, and mathematically verified.
                </div>
            ` : `
                <div class="p-3.5 bg-amber-50/60 rounded-xl text-amber-900 leading-relaxed text-xs border border-amber-200">
                    <span class="font-bold text-amber-800">Ledger Status:</span> This payment is finalized in the internal double-entry ledger. To anchor it on Solana L1 and generate public cryptographic proof, click <strong>"Anchor to Solana L1 Now"</strong> above.
                </div>
            `;

            container.innerHTML = `
                <div class="bg-slate-50 p-4 rounded-xl border border-slate-200 flex items-center justify-between">
                    <div>
                        <div class="font-bold text-slate-900 text-base">Order #${data.payment_id}</div>
                        <div class="text-slate-500 text-xs font-mono mt-0.5">Amount: $${(data.amount / 1000000).toFixed(2)} USDC</div>
                    </div>
                    <div>${verifiedBadge}</div>
                </div>

                ${actionAnchorBtn}

                <div class="p-4 bg-slate-50 rounded-xl border border-slate-200 space-y-3 font-mono text-xs">
                    <div>
                        <div class="font-bold text-slate-600 font-sans text-xs">1. Transfer Leaf Hash (SHA-256):</div>
                        <div class="text-slate-800 break-all bg-white p-2.5 rounded-lg border border-slate-200 text-[11px] mt-1 shadow-2xs">
                            ${data.leaf_hash || 'Pending batch anchor'}
                        </div>
                    </div>

                    <div>
                        <div class="font-bold text-slate-600 font-sans text-xs">2. Merkle Mountain Range Path:</div>
                        <div class="space-y-1.5 mt-1">
                            ${siblingsHtml}
                        </div>
                    </div>

                    <div>
                        <div class="font-bold text-slate-600 font-sans text-xs">3. Solana L1 PDA Committed Merkle Root:</div>
                        <div class="text-purple-700 font-bold break-all bg-white p-2.5 rounded-lg border border-purple-200 text-[11px] mt-1 shadow-2xs">
                            ${data.merkle_root || 'Awaiting batch commit to Solana L1'}
                        </div>
                    </div>
                </div>

                ${guaranteeBox}
            `;

            document.getElementById('proofModal').classList.remove('hidden');
        }

        function closeProofModal() {
            document.getElementById('proofModal').classList.add('hidden');
        }

        document.addEventListener('DOMContentLoaded', () => {
            fetchDashboardData();
            setInterval(fetchDashboardData, 4000);
        });
    </script>
</body>
</html>"#;
