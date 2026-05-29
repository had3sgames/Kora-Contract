use soroban_sdk::{contracttype, Env};
use crate::errors::KoraError;

/// Storage key for the reentrancy guard.
/// Uses key presence as the lock signal — no boolean value is stored.
#[contracttype]
pub enum GuardKey {
    ReentrancyGuard,
}

/// Acquire a reentrancy guard.
///
/// Returns `KoraError::Reentrancy` if the guard is already held, indicating
/// a recursive call into a protected function within the same transaction.
/// Must be paired with a `release_guard` call on every exit path.
pub fn acquire_guard(env: &Env) -> Result<(), KoraError> {
    if env.storage().instance().has(&GuardKey::ReentrancyGuard) {
        return Err(KoraError::Reentrancy);
    }
    // Store unit value — only the key's presence matters
    env.storage().instance().set(&GuardKey::ReentrancyGuard, &());
    Ok(())
}

/// Release the reentrancy guard by removing the key.
/// Must be called before returning from every protected function.
pub fn release_guard(env: &Env) {
    env.storage().instance().remove(&GuardKey::ReentrancyGuard);
}

/// RAII-style guard that automatically releases on drop.
///
/// Preferred over manual acquire/release pairs because it guarantees
/// the guard is released even if the function returns early via `?`.
pub struct ReentrancyGuard<'a> {
    env: &'a Env,
}

impl<'a> ReentrancyGuard<'a> {
    pub fn new(env: &'a Env) -> Result<Self, KoraError> {
        acquire_guard(env)?;
        Ok(ReentrancyGuard { env })
    }
}

impl<'a> Drop for ReentrancyGuard<'a> {
    fn drop(&mut self) {
        release_guard(self.env);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn test_guard_acquire_release() {
        let env = Env::default();
        assert!(acquire_guard(&env).is_ok());
        // Second acquire must fail with Reentrancy, not Unauthorized
        let err = acquire_guard(&env).unwrap_err();
        assert_eq!(err, KoraError::Reentrancy);
        release_guard(&env);
        assert!(acquire_guard(&env).is_ok());
    }

    #[test]
    fn test_raii_guard() {
        let env = Env::default();
        {
            let _guard = ReentrancyGuard::new(&env).unwrap();
            let err = ReentrancyGuard::new(&env).unwrap_err();
            assert_eq!(err, KoraError::Reentrancy);
        }
        // Guard released on drop — should succeed now
        assert!(ReentrancyGuard::new(&env).is_ok());
    }

    #[test]
    fn test_release_without_acquire_is_safe() {
        let env = Env::default();
        // Releasing when not locked should not panic
        release_guard(&env);
        // And acquiring afterwards should succeed
        assert!(acquire_guard(&env).is_ok());
    }
}
