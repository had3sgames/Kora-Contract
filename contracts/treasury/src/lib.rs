#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    validation::{require_non_zero_amount, require_valid_fee_bps},
};
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env};

// ── Storage TTL constants (~31 days in ledgers) ───────────────────────────────
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Protocol fee in basis points — persistent for durability.
    FeeBps,
    /// Accumulated fees per token (informational accounting).
    Collected(Address),
    /// Whitelisted token addresses — only these can be withdrawn.
    WhitelistedToken(Address),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    /// One-time initializer. Sets admin and initial fee rate.
    /// All config is stored in persistent storage for durability across ledger archival.
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        // Guard: prevent re-initialization
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;

        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);

        Self::bump_persistent(&env, &DataKey::Admin);
        Self::bump_persistent(&env, &DataKey::FeeBps);

        events::treasury_initialized(&env, &admin, fee_bps);
        Ok(())
    }

    /// Update protocol fee. Admin only.
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_valid_fee_bps(fee_bps)?;

        // Acquire reentrancy guard for the duration of this state mutation
        let _guard = ReentrancyGuard::new(&env)?;

        let old_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50);

        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);
        Self::bump_persistent(&env, &DataKey::FeeBps);

        events::fee_rate_updated(&env, &admin, old_bps, fee_bps);
        Ok(())
    }

    /// Whitelist a token so it can be used in withdraw / emergency_withdraw.
    /// Admin only.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(&env, &DataKey::WhitelistedToken(token.clone()));

        events::token_whitelisted(&env, &token);
        Ok(())
    }

    /// Record an incoming fee for a given token. Called by the marketplace after
    /// transferring the fee amount to this contract. Updates the informational
    /// accounting ledger.
    ///
    /// No auth required — the token transfer itself is the proof of payment.
    /// The amount is validated to be > 0 to prevent no-op accounting entries.
    pub fn collect_fee(env: Env, token: Address, amount: i128) -> Result<(), KoraError> {
        require_non_zero_amount(amount)?;
        Self::require_whitelisted_token(&env, &token)?;

        let key = DataKey::Collected(token.clone());
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        let new_total = current
            .checked_add(amount)
            .ok_or(KoraError::ArithmeticOverflow)?;

        env.storage().persistent().set(&key, &new_total);
        Self::bump_persistent(&env, &key);

        events::fee_collected(&env, 0, amount, &token);
        Ok(())
    }

    /// Withdraw accumulated fees to a recipient. Admin only.
    /// Protected against reentrancy via the shared RAII guard.
    /// Follows checks-effects-interactions: all validation before external call.
    pub fn withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), KoraError> {
        // ── Checks ────────────────────────────────────────────────────────────
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_non_zero_amount(amount)?;
        Self::require_whitelisted_token(&env, &token)?;

        // Acquire reentrancy guard — released automatically when _guard drops
        let _guard = ReentrancyGuard::new(&env)?;

        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());

        if balance < amount {
            return Err(KoraError::InsufficientPoolBalance);
        }

        // ── Effects ───────────────────────────────────────────────────────────
        // Deduct from informational accounting if tracked
        let collected_key = DataKey::Collected(token.clone());
        if let Some(collected) = env
            .storage()
            .persistent()
            .get::<_, i128>(&collected_key)
        {
            // Saturating sub: accounting is informational, don't revert on mismatch
            let new_collected = collected.saturating_sub(amount);
            env.storage()
                .persistent()
                .set(&collected_key, &new_collected);
            Self::bump_persistent(&env, &collected_key);
        }

        // ── Interactions ──────────────────────────────────────────────────────
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        events::fee_withdrawn(&env, &token, amount);
        Ok(())
    }

    /// Emergency drain — withdraw entire token balance. Admin only.
    /// Protected against reentrancy via the shared RAII guard.
    pub fn emergency_withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
    ) -> Result<(), KoraError> {
        // ── Checks ────────────────────────────────────────────────────────────
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_whitelisted_token(&env, &token)?;

        // Acquire reentrancy guard — released automatically when _guard drops
        let _guard = ReentrancyGuard::new(&env)?;

        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());

        if balance == 0 {
            // Nothing to drain — return early (guard drops cleanly)
            return Ok(());
        }

        // ── Effects ───────────────────────────────────────────────────────────
        // Zero out the informational accounting for this token
        let collected_key = DataKey::Collected(token.clone());
        if env.storage().persistent().has(&collected_key) {
            env.storage().persistent().set(&collected_key, &0i128);
            Self::bump_persistent(&env, &collected_key);
        }

        // ── Interactions ──────────────────────────────────────────────────────
        token_client.transfer(&env.current_contract_address(), &recipient, &balance);

        events::emergency_withdrawn(&env, &admin, &token, balance);
        Ok(())
    }

    /// Returns the current protocol fee in basis points.
    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50)
    }

    /// Returns the live token balance held by this contract.
    pub fn get_balance(env: Env, token: Address) -> i128 {
        token::Client::new(&env, &token).balance(&env.current_contract_address())
    }

    /// Returns the informational accumulated fee total for a token.
    pub fn get_collected(env: Env, token: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Collected(token))
            .unwrap_or(0)
    }

    /// Returns whether a token is whitelisted.
    pub fn is_token_whitelisted(env: Env, token: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::WhitelistedToken(token))
            .unwrap_or(false)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)?;
        if &admin != caller {
            return Err(KoraError::NotAdmin);
        }
        Ok(())
    }

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let whitelisted: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !whitelisted {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage().persistent().extend_ttl(
            key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }
}
