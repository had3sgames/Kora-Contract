# Kora Protocol — Architecture

This document describes the technical architecture of the Kora Protocol: how the contracts are structured, how they interact, and the design decisions behind them.

---

## Design Principles

**Separation of concerns.** Each contract owns exactly one domain. The Invoice NFT contract owns invoice state. The Marketplace owns listing and funding logic. The Financing Pool owns fund custody and yield distribution. No contract reaches into another's storage directly — all cross-contract interaction is via explicit function calls.

**Explicit state machines.** Invoice status transitions are strictly enforced. An invoice can only move forward through its lifecycle (`Created → Listed → Funded → Repaid | Defaulted`). Backward transitions are impossible by construction.

**Minimal on-chain footprint.** Debtor PII is never stored on-chain. Only a SHA-256 hash of debtor information is stored. Full invoice metadata lives on IPFS, referenced by CID. This keeps storage costs low and protects privacy.

**Safe arithmetic everywhere.** All financial calculations use Rust's `checked_*` methods. Any overflow returns a typed `KoraError::ArithmeticOverflow` rather than silently wrapping or panicking.

**Upgrade-safe storage.** Storage keys are defined as `#[contracttype]` enums. Adding new variants is non-breaking. Existing keys are never reused for different data types.

---

## Contract Map

```
┌─────────────────────────────────────────────────────────────────┐
│                        Kora Protocol                            │
│                                                                 │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────┐  │
│  │ access_control│    │ risk_registry│    │     treasury     │  │
│  │              │    │              │    │                  │  │
│  │ pause/unpause│    │ SME profiles │    │ fee accumulation │  │
│  │ role mgmt    │    │ verifiers    │    │ admin withdrawal │  │
│  │ admin xfer   │    │ debtor scores│    │                  │  │
│  └──────┬───────┘    └──────────────┘    └────────┬─────────┘  │
│         │                                          │            │
│         │ (pause check)              (fee transfer)│            │
│         ▼                                          │            │
│  ┌──────────────┐    ┌──────────────┐    ┌────────▼─────────┐  │
│  │  invoice_nft │◄───│  marketplace │───►│  financing_pool  │  │
│  │              │    │              │    │                  │  │
│  │ mint NFT     │    │ list invoice │    │ hold funds       │  │
│  │ state machine│    │ fund invoice │    │ track positions  │  │
│  │ get invoice  │    │ cancel       │    │ repay + yield    │  │
│  └──────────────┘    │ fee collect  │    │ default handling │  │
│                      └──────────────┘    └──────────────────┘  │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                        shared                            │  │
│  │  types · errors · events · validation                    │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Contract Responsibilities

### `shared`

A library crate (not a deployable contract). Provides:

- **`types`** — all shared data structures: `Invoice`, `Listing`, `Pool`, `Position`, `SmeProfile`, `ProtocolConfig`, `InvoiceStatus`, `RiskTier`
- **`errors`** — the `KoraError` enum used across all contracts
- **`events`** — all protocol event emission functions (single source of truth for event names)
- **`validation`** — reusable guards: `require_non_zero_amount`, `require_future_timestamp`, `bps_of`, etc.

### `invoice_nft`

The canonical source of truth for invoice state.

**Storage:**
- `Invoice(u64)` → `Invoice` struct (persistent)
- `NextId` → `u64` (instance)
- `Admin` → `Address` (instance)
- `AccessControl` → `Address` (instance)

**Key invariants:**
- Only the contract itself (via authorized callers) can transition invoice status
- `set_listed` requires the marketplace contract's auth
- `set_funded` requires the financing pool's auth
- `set_repaid` requires the financing pool's auth
- `set_defaulted` requires admin auth AND `ledger.timestamp > due_date`

### `marketplace`

Manages the listing lifecycle and investor funding flow.

**Storage:**
- `Listing(u64)` → `Listing` struct (persistent)
- `WhitelistedToken(Address)` → `bool` (persistent)
- `Admin`, `InvoiceNft`, `FinancingPool`, `Treasury`, `FeeBps` (instance)

**Fee flow:**
```
investor pays amount
  ├── fee = amount × fee_bps / 10_000  →  treasury
  └── net = amount - fee               →  financing_pool
```

When `funded_amount >= asking_price`, the listing is closed and `financing_pool.release_funds()` is called.

### `financing_pool`

Custodian of investor funds. Handles repayment and yield distribution.

**Storage:**
- `Pool(u64)` → `Pool` struct (persistent)
- `Positions(u64)` → `Map<Address, Position>` (persistent)
- `Admin`, `InvoiceNft`, `Treasury`, `LatePenaltyBps` (instance)

**Yield calculation:**
Each investor's payout is proportional to their share of the pool:

```
share_bps = (contributed / total_funded) × 10_000
payout    = (total_repaid × share_bps) / 10_000
yield     = payout - contributed
```

### `treasury`

Simple fee accumulator with admin-controlled withdrawal.

**Storage:**
- `Admin`, `FeeBps` (instance)
- `Collected(Address)` → `i128` per token (persistent, informational)

The treasury holds no special logic — it is a standard Stellar account that receives token transfers from the marketplace. The `withdraw` and `emergency_withdraw` functions allow the admin to move funds out.

### `risk_registry`

Verifier-managed SME and debtor scoring.

**Storage:**
- `Verifier(Address)` → `bool` (persistent)
- `SmeProfile(Address)` → `SmeProfile` (persistent)
- `DebtorScore(Bytes)` → `u32` (persistent, keyed by debtor hash)
- `Admin` (instance)

Verifiers are trusted off-chain entities (e.g., credit bureaus, KYC providers) who have been whitelisted by the admin. They assign risk scores to SMEs and debtors. The marketplace can optionally gate listings based on minimum risk score.

### `access_control`

Protocol-wide pause switch and role registry.

**Storage:**
- `Admin` → `Address` (instance)
- `Paused` → `bool` (instance)
- `Role(Address)` → `Role` (persistent)

Roles: `Admin`, `Operator`, `Verifier`, `None`.

The pause flag is read by other contracts via cross-contract call. When paused, all state-mutating operations revert with `KoraError::ProtocolPaused`.

---

## Invoice State Machine

```
                    ┌─────────┐
                    │ Created │  ← mint_invoice()
                    └────┬────┘
                         │ set_listed() [marketplace auth]
                    ┌────▼────┐
                    │ Listed  │
                    └────┬────┘
                         │ set_funded() [pool auth]
                    ┌────▼────┐
                    │ Funded  │
                    └────┬────┘
           ┌─────────────┴─────────────┐
           │ set_repaid()              │ set_defaulted()
           │ [pool auth]               │ [admin auth + past due_date]
      ┌────▼────┐                 ┌────▼──────┐
      │ Repaid  │                 │ Defaulted │
      └─────────┘                 └───────────┘
```

Transitions are enforced in `invoice_nft`. Any attempt to skip a step or go backward returns `KoraError::InvalidInvoiceStatus`.

---

## Cross-Contract Call Graph

```
marketplace.list_invoice()
  └── invoice_nft.set_listed()

marketplace.fund_invoice()
  ├── token.transfer(investor → treasury)   [fee]
  ├── token.transfer(investor → pool)       [net]
  └── [if fully funded] financing_pool.release_funds()
        └── invoice_nft.set_funded()

financing_pool.repay()
  ├── token.transfer(payer → pool)
  ├── [if fully repaid] distribute_yield()
  │     └── token.transfer(pool → each investor)
  └── [if fully repaid] invoice_nft.set_repaid()

invoice_nft.set_defaulted()
  └── [called by admin directly]
```

---

## Storage Layout

All contracts use Soroban's three storage tiers:

| Tier | Used for | TTL |
|------|----------|-----|
| `instance` | Contract-level config (admin, addresses, flags) | Tied to contract instance |
| `persistent` | Per-entity data (invoices, listings, pools, profiles) | Explicitly managed |
| `temporary` | Not used in v1 | — |

Persistent entries must have their TTL extended periodically (via `extend_ttl`) to avoid expiry. This is the responsibility of the protocol operator or a keeper bot.

---

## Fee Model

```
Investor contribution:  1,000 USDC
Marketplace fee (0.5%):     5 USDC  → treasury
Net to pool:              995 USDC  → financing_pool

SME receives:             995 USDC  (net of fee)
SME repays:             1,000 USDC  (face value)

Investor yield:             5 USDC  (spread between net paid and face value received)
```

The fee is taken at funding time, not at repayment. This means the protocol earns revenue regardless of whether the invoice is repaid.

---

## Security Architecture

See [SECURITY.md](SECURITY.md) for the full security model. Key points:

- Every state-mutating function calls `require_auth()` on the relevant signer before any logic executes.
- Cross-contract calls use the calling contract's address as the authorized signer — this is verified by the callee.
- No `unwrap()` in contract code. All fallible operations return `Result<_, KoraError>`.
- No floating-point arithmetic. All financial math uses integer basis points.
- The `shared::validation::bps_of` function is the single implementation of basis-point math — used everywhere fees or shares are calculated.

---

## Upgrade Path

Soroban contracts are upgradeable via `contract.upgrade(new_wasm_hash)`. The upgrade function is not yet implemented in v1 — it will be added in v2 with a timelock and multisig requirement.

For v1, upgrades require redeployment and migration of state. The deployment manifest (`deployments/<network>.json`) tracks all contract addresses to facilitate migration scripts.

---

## Future Work

- **Timelock on admin actions** — delay sensitive operations by 48h
- **Multisig admin** — replace single admin with a threshold signature scheme
- **Secondary market** — allow investors to trade their pool positions
- **Oracle integration** — on-chain FX rates for multi-currency invoices
- **Reputation NFTs** — on-chain track record for SMEs with strong repayment history
- **Keeper network** — automated TTL extension and default detection
