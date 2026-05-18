# Kora Protocol — Security Model

This document describes the security architecture of the Kora Protocol, the threat model it is designed against, and the controls in place to mitigate each risk.

---

## Threat Model

Kora is a financial protocol. The primary threats are:

1. **Unauthorized state mutation** — an attacker modifies invoice status, pool balances, or fee rates without permission
2. **Fund theft** — an attacker drains the financing pool or treasury
3. **Arithmetic exploits** — overflow or underflow in fee or yield calculations leads to incorrect fund distribution
4. **Griefing** — an attacker prevents legitimate users from interacting with the protocol
5. **Admin key compromise** — the admin private key is stolen, giving an attacker full protocol control
6. **PII exposure** — debtor personal information is leaked on-chain

---

## Controls

### Authentication

Every state-mutating function calls `require_auth()` on the relevant signer as the **first operation** before any logic executes. This is enforced by the Soroban runtime — if the auth check fails, the entire transaction is reverted with no state changes.

```rust
// Pattern used in every mutating function
pub fn some_function(env: Env, caller: Address, ...) -> Result<(), KoraError> {
    caller.require_auth();  // ← always first
    // ... logic follows
}
```

Cross-contract calls use the calling contract's address as the authorized signer. The callee verifies this address matches the expected contract (e.g., `invoice_nft.set_listed` checks that the caller is the marketplace contract).

### Role-Based Access Control

| Role | Capabilities |
|------|-------------|
| Admin | Pause/unpause, set fees, whitelist tokens, add/remove verifiers, emergency withdrawal, mark defaults, transfer admin |
| Operator | Reserved for future use (e.g., keeper bots) |
| Verifier | Register SMEs, update risk scores, set debtor scores |
| None | No privileged access |

Roles are stored in `access_control` and checked by each contract independently. There is no global role lookup — each contract enforces its own access rules.

### Protocol Pause

The `access_control` contract exposes a `paused` flag. When set, all state-mutating operations in `invoice_nft` and `marketplace` revert with `KoraError::ProtocolPaused`. This allows the admin to halt the protocol in response to a discovered vulnerability without requiring contract upgrades.

The pause does **not** block repayments — SMEs can always repay their invoices even when the protocol is paused.

### Safe Arithmetic

All financial calculations use Rust's `checked_*` methods:

```rust
// From shared/src/validation.rs
pub fn bps_of(amount: i128, bps: u32) -> Result<i128, KoraError> {
    amount
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(KoraError::ArithmeticOverflow)
}
```

There is no floating-point arithmetic anywhere in the protocol. All percentages are expressed in basis points (integers). Overflow returns `KoraError::ArithmeticOverflow` and reverts the transaction.

### Input Validation

All public entry points validate inputs before any state changes:

- Amounts must be > 0
- Timestamps must be in the future
- Risk scores must be 0–100
- Strings and byte arrays must be non-empty
- Fee rates must be ≤ 10,000 bps (100%)
- Token addresses must be whitelisted

Validation is centralized in `shared/src/validation.rs` to ensure consistency.

### PII Protection

Debtor personal information (name, company, address, tax ID) is **never stored on-chain**. Only a SHA-256 hash of the debtor information is stored in the `Invoice` struct. Full details are stored on IPFS and referenced by CID. The IPFS content should be encrypted and access-controlled by the SME.

### Reentrancy

Soroban's execution model is synchronous and single-threaded within a transaction. There is no async callback mechanism that would enable classic reentrancy attacks. However, the protocol follows the checks-effects-interactions pattern as a defense-in-depth measure: all state is updated before any token transfers are made.

### Storage Key Safety

Storage keys are defined as `#[contracttype]` enums. This prevents key collisions between different data types. New storage keys can be added in future versions without conflicting with existing keys.

---

## Known Limitations (v1)

### Single Admin Key

The admin is a single Stellar keypair. If this key is compromised, an attacker has full protocol control. Mitigations planned for v2:

- Multisig admin (threshold signature)
- Timelock on sensitive admin operations (48h delay)
- Admin key stored in hardware security module

### No Upgrade Mechanism

v1 contracts are not upgradeable. A critical bug requires redeployment and state migration. An upgrade mechanism with timelock and multisig will be added in v2.

### No Oracle

Invoice amounts and due dates are self-reported by SMEs. There is no on-chain oracle to verify that the underlying invoice is real. This is mitigated off-chain by the verifier network — verifiers are responsible for KYC/KYB and invoice authenticity checks before assigning a risk score.

### TTL Management

Soroban persistent storage entries expire if their TTL is not extended. In v1, TTL extension is a manual operation. A keeper bot or protocol operator must periodically call `extend_ttl` on active invoice and pool entries. Failure to do so could result in data loss.

---

## Audit Status

Kora Protocol v1 has not yet been audited. **Do not deploy to mainnet with real funds until a professional audit has been completed.**

Planned audit scope:
- All 6 contracts
- Cross-contract interaction patterns
- Fee and yield calculation correctness
- Access control completeness
- Storage layout and TTL handling

---

## Responsible Disclosure

Report security vulnerabilities privately to **security@kora.finance**.

Do not open public GitHub issues for security vulnerabilities. We will acknowledge within 48 hours and aim to patch critical issues within 7 days.

See [CONTRIBUTING.md](../CONTRIBUTING.md#security-vulnerabilities) for the full disclosure policy.
