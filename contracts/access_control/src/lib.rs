#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Env,
};
use kora_shared::{errors::KoraError, events};

// ── Storage TTL constants ─────────────────────────────────────────────────────
// Persistent storage entries must have their TTL bumped to stay live.
// These values are in ledgers (roughly 5s each on Stellar mainnet).
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680; // ~31 days
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2; // bump when below half

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Protocol pause flag — persistent so pause state is never silently lost.
    Paused,
    /// Per-address role mapping.
    Role(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Admin,
    Operator,
    Verifier,
    None,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct AccessControlContract;

#[contractimpl]
impl AccessControlContract {
    pub fn initialize(env: Env, admin: Address) -> Result<(), KoraError> {
        // Guard: prevent re-initialization
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }

        // Validate admin address is not the zero/contract address
        // (soroban Address type is always valid, but we record it)
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Paused, &false);
        env.storage().persistent().set(&DataKey::Role(admin.clone()), &Role::Admin);

        // Bump TTL so these entries don't expire
        env.storage().persistent().extend_ttl(
            &DataKey::Admin,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::Paused,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::Role(admin),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Ok(())
    }

    /// Pause the entire protocol. Admin only.
    pub fn pause(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if Self::read_paused(&env) {
            return Err(KoraError::ProtocolPaused);
        }

        env.storage().persistent().set(&DataKey::Paused, &true);
        env.storage().persistent().extend_ttl(
            &DataKey::Paused,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::protocol_paused(&env, &admin);
        Ok(())
    }

    /// Unpause the protocol. Admin only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if !Self::read_paused(&env) {
            // Already unpaused — use a distinct, accurate error
            return Err(KoraError::NotPaused);
        }

        env.storage().persistent().set(&DataKey::Paused, &false);
        env.storage().persistent().extend_ttl(
            &DataKey::Paused,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::protocol_unpaused(&env, &admin);
        Ok(())
    }

    /// Assign a role to an address. Admin only.
    /// Granting `Role::Admin` is forbidden — use `transfer_admin` instead.
    pub fn grant_role(
        env: Env,
        admin: Address,
        target: Address,
        role: Role,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if role == Role::Admin {
            return Err(KoraError::Unauthorized);
        }

        env.storage().persistent().set(&DataKey::Role(target.clone()), &role);
        env.storage().persistent().extend_ttl(
            &DataKey::Role(target),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Ok(())
    }

    /// Revoke a role from an address. Admin only.
    /// Removes the storage entry entirely rather than writing `Role::None`.
    pub fn revoke_role(env: Env, admin: Address, target: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let current_role = env
            .storage()
            .persistent()
            .get::<_, Role>(&DataKey::Role(target.clone()))
            .unwrap_or(Role::None);

        if current_role == Role::Admin {
            return Err(KoraError::Unauthorized);
        }

        // Remove the entry rather than writing Role::None — saves storage rent
        env.storage().persistent().remove(&DataKey::Role(target));

        Ok(())
    }

    /// Transfer admin to a new address. Current admin must sign.
    pub fn transfer_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
    ) -> Result<(), KoraError> {
        current_admin.require_auth();
        Self::require_admin(&env, &current_admin)?;

        if current_admin == new_admin {
            return Err(KoraError::InvalidAddress);
        }

        env.storage().persistent().set(&DataKey::Admin, &new_admin);
        env.storage().persistent().set(&DataKey::Role(new_admin.clone()), &Role::Admin);
        // Remove old admin's role entry
        env.storage().persistent().remove(&DataKey::Role(current_admin));

        env.storage().persistent().extend_ttl(
            &DataKey::Admin,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::Role(new_admin.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::admin_transferred(&env, &new_admin);
        Ok(())
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn is_paused(env: Env) -> bool {
        Self::read_paused(&env)
    }

    pub fn get_role(env: Env, address: Address) -> Role {
        env.storage()
            .persistent()
            .get(&DataKey::Role(address))
            .unwrap_or(Role::None)
    }

    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Read the paused flag from persistent storage.
    fn read_paused(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, AccessControlContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    #[test]
    fn test_pause_unpause() {
        let (_, admin, client) = setup();
        assert!(!client.is_paused());
        client.pause(&admin);
        assert!(client.is_paused());
        client.unpause(&admin);
        assert!(!client.is_paused());
    }

    #[test]
    fn test_pause_already_paused_fails() {
        let (_, admin, client) = setup();
        client.pause(&admin);
        let result = client.try_pause(&admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_unpause_when_not_paused_fails() {
        let (_, admin, client) = setup();
        // Not paused yet — unpause should return NotPaused
        let result = client.try_unpause(&admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_grant_revoke_role() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);

        client.grant_role(&admin, &operator, &Role::Operator);
        assert_eq!(client.get_role(&operator), Role::Operator);

        client.revoke_role(&admin, &operator);
        // After revoke the entry is removed — should return None
        assert_eq!(client.get_role(&operator), Role::None);
    }

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);

        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.get_admin(), new_admin);
        assert_eq!(client.get_role(&new_admin), Role::Admin);
        // Old admin role entry removed
        assert_eq!(client.get_role(&admin), Role::None);
    }

    #[test]
    fn test_transfer_admin_same_address_fails() {
        let (_, admin, client) = setup();
        let result = client.try_transfer_admin(&admin, &admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_non_admin_cannot_pause() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let result = client.try_pause(&stranger);
        assert!(result.is_err());
    }

    #[test]
    fn test_non_admin_cannot_grant_role() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_grant_role(&stranger, &target, &Role::Verifier);
        assert!(result.is_err());
    }

    #[test]
    fn test_non_admin_cannot_transfer_admin() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&stranger, &new_admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_grant_admin_role_forbidden() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        // Granting Admin role via grant_role must be rejected
        let result = client.try_grant_role(&admin, &target, &Role::Admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_revoke_admin_role_forbidden() {
        let (env, admin, client) = setup();
        // Revoking the admin's own role must be rejected
        let result = client.try_revoke_role(&admin, &admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_role_assignments() {
        let (env, admin, client) = setup();
        let verifier1 = Address::generate(&env);
        let verifier2 = Address::generate(&env);
        let operator = Address::generate(&env);

        client.grant_role(&admin, &verifier1, &Role::Verifier);
        client.grant_role(&admin, &verifier2, &Role::Verifier);
        client.grant_role(&admin, &operator, &Role::Operator);

        assert_eq!(client.get_role(&verifier1), Role::Verifier);
        assert_eq!(client.get_role(&verifier2), Role::Verifier);
        assert_eq!(client.get_role(&operator), Role::Operator);
    }

    #[test]
    fn test_role_override() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);

        client.grant_role(&admin, &user, &Role::Operator);
        assert_eq!(client.get_role(&user), Role::Operator);

        // Override with different role
        client.grant_role(&admin, &user, &Role::Verifier);
        assert_eq!(client.get_role(&user), Role::Verifier);
    }

    #[test]
    fn test_initialize_already_initialized_fails() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin);
        assert!(result.is_err());
    }
}
