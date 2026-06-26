use soroban_sdk::{Address, Bytes, Env, String};
use crate::errors::KoraError;

pub fn require_non_zero_amount(amount: i128) -> Result<(), KoraError> {
    if amount <= 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

pub fn require_positive_amount(amount: i128) -> Result<(), KoraError> {
    if amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

pub fn require_future_timestamp(env: &Env, ts: u64) -> Result<(), KoraError> {
    if ts <= env.ledger().timestamp() {
        return Err(KoraError::InvalidDueDate);
    }
    Ok(())
}

pub fn require_valid_risk_score(score: u32) -> Result<(), KoraError> {
    if score > 100 {
        return Err(KoraError::InvalidRiskScore);
    }
    Ok(())
}

pub fn require_non_empty_string(s: &String) -> Result<(), KoraError> {
    if s.len() == 0 {
        return Err(KoraError::EmptyString);
    }
    Ok(())
}

pub fn require_non_empty_bytes(b: &Bytes) -> Result<(), KoraError> {
    if b.len() == 0 {
        return Err(KoraError::EmptyString);
    }
    Ok(())
}

pub fn require_valid_fee_bps(bps: u32) -> Result<(), KoraError> {
    if bps > 10_000 {
        return Err(KoraError::InvalidFeeRate);
    }
    Ok(())
}

pub fn require_amount_within_bounds(amount: i128, max: i128) -> Result<(), KoraError> {
    if amount > max || amount < 0 {
        return Err(KoraError::InvalidAmount);
    }
    Ok(())
}

/// Safe basis-point multiplication: (amount * bps) / 10_000
pub fn bps_of(amount: i128, bps: u32) -> Result<i128, KoraError> {
    amount
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(KoraError::ArithmeticOverflow)
}

/// Safe addition with overflow check
pub fn safe_add(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_add(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Safe subtraction with underflow check
pub fn safe_sub(a: i128, b: i128) -> Result<i128, KoraError> {
    a.checked_sub(b).ok_or(KoraError::ArithmeticOverflow)
}

/// Reject the contract's own address being passed as a counterparty or admin.
/// Prevents self-referential configuration bugs (e.g. admin == contract itself).
pub fn require_not_self(env: &Env, addr: &Address) -> Result<(), KoraError> {
    if addr == &env.current_contract_address() {
        return Err(KoraError::InvalidAddress);
    }
    Ok(())
}

/// Reject two addresses being identical where they must be distinct
/// (e.g. admin == treasury, or two different contract addresses colliding).
pub fn require_distinct(a: &Address, b: &Address) -> Result<(), KoraError> {
    if a == b {
        return Err(KoraError::InvalidAddress);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn test_require_non_zero_amount() {
        assert!(require_non_zero_amount(0).is_err());
        assert!(require_non_zero_amount(-1).is_err());
        assert!(require_non_zero_amount(1).is_ok());
    }

    #[test]
    fn test_require_positive_amount() {
        assert!(require_positive_amount(-1).is_err());
        assert!(require_positive_amount(0).is_ok());
        assert!(require_positive_amount(1).is_ok());
    }

    #[test]
    fn test_bps_of_safe() {
        assert_eq!(bps_of(10_000, 100).unwrap(), 100);
        assert_eq!(bps_of(1_000_000, 50).unwrap(), 5_000);
        assert!(bps_of(i128::MAX, 10_000).is_err());
    }

    #[test]
    fn test_safe_add() {
        assert_eq!(safe_add(100, 200).unwrap(), 300);
        assert!(safe_add(i128::MAX, 1).is_err());
    }

    #[test]
    fn test_safe_sub() {
        assert_eq!(safe_sub(300, 100).unwrap(), 200);
        assert!(safe_sub(100, 200).is_err());
    }

    #[test]
    fn test_require_not_self() {
        let env = Env::default();
        let self_addr = env.current_contract_address();
        let other = soroban_sdk::Address::from_contract_id(&soroban_sdk::BytesN::from_array(&env, &[1u8; 32]));
        assert!(require_not_self(&env, &self_addr).is_err());
        assert!(require_not_self(&env, &other).is_ok());
    }

    #[test]
    fn test_require_distinct() {
        let env = Env::default();
        let a = soroban_sdk::Address::from_contract_id(&soroban_sdk::BytesN::from_array(&env, &[1u8; 32]));
        let b = soroban_sdk::Address::from_contract_id(&soroban_sdk::BytesN::from_array(&env, &[2u8; 32]));
        assert!(require_distinct(&a, &a).is_err());
        assert!(require_distinct(&a, &b).is_ok());
    }
}
