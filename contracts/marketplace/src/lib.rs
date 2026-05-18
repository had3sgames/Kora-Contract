#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Env,
};
use kora_shared::{
    errors::KoraError,
    events,
    types::{Listing},
    validation::{bps_of, require_non_zero_amount},
};

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Listing(u64),
    Admin,
    InvoiceNft,
    FinancingPool,
    Treasury,
    FeeBps,
    WhitelistedToken(Address),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct MarketplaceContract;

#[contractimpl]
impl MarketplaceContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_nft: Address,
        financing_pool: Address,
        treasury: Address,
        fee_bps: u32,
    ) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        kora_shared::validation::require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        Ok(())
    }

    /// SME lists an invoice NFT for financing.
    pub fn list_invoice(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
    ) -> Result<(), KoraError> {
        seller.require_auth();
        require_non_zero_amount(asking_price)?;
        require_non_zero_amount(face_value)?;
        kora_shared::validation::require_future_timestamp(&env, funding_deadline)?;

        if asking_price >= face_value {
            return Err(KoraError::InvalidAmount); // discount must exist
        }
        Self::require_whitelisted_token(&env, &token)?;

        if env.storage().persistent().has(&DataKey::Listing(invoice_id)) {
            return Err(KoraError::InvoiceAlreadyExists);
        }

        // Notify Invoice NFT contract to transition status
        let nft_contract: Address = env.storage().instance().get(&DataKey::InvoiceNft).unwrap();
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        nft_client.set_listed(&env.current_contract_address(), &invoice_id);

        let listing = Listing {
            invoice_id,
            seller: seller.clone(),
            asking_price,
            face_value,
            token,
            funded_amount: 0,
            funding_deadline,
            is_active: true,
        };
        env.storage().persistent().set(&DataKey::Listing(invoice_id), &listing);
        events::invoice_listed(&env, invoice_id, &seller, asking_price);
        Ok(())
    }

    /// Investor funds a share of the invoice.
    pub fn fund_invoice(
        env: Env,
        investor: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), KoraError> {
        investor.require_auth();
        require_non_zero_amount(amount)?;

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }
        if env.ledger().timestamp() > listing.funding_deadline {
            return Err(KoraError::FundingDeadlinePassed);
        }

        let remaining = listing.asking_price
            .checked_sub(listing.funded_amount)
            .ok_or(KoraError::ArithmeticOverflow)?;

        if amount > remaining {
            return Err(KoraError::ExceedsFundingTarget);
        }

        // Collect marketplace fee from investor
        let fee_bps: u32 = env.storage().instance().get(&DataKey::FeeBps).unwrap_or(50);
        let fee = bps_of(amount, fee_bps)?;
        let net = amount.checked_sub(fee).ok_or(KoraError::ArithmeticOverflow)?;

        let token_client = token::Client::new(&env, &listing.token);
        let treasury: Address = env.storage().instance().get(&DataKey::Treasury).unwrap();

        // Transfer fee to treasury
        token_client.transfer(&investor, &treasury, &fee);
        // Transfer net to financing pool
        let pool_contract: Address = env.storage().instance().get(&DataKey::FinancingPool).unwrap();
        token_client.transfer(&investor, &pool_contract, &net);

        listing.funded_amount = listing
            .funded_amount
            .checked_add(amount)
            .ok_or(KoraError::ArithmeticOverflow)?;

        let fully_funded = listing.funded_amount >= listing.asking_price;
        if fully_funded {
            listing.is_active = false;
        }

        env.storage().persistent().set(&DataKey::Listing(invoice_id), &listing);
        events::invoice_funded(&env, invoice_id, &investor, amount);
        events::fee_collected(&env, invoice_id, fee, &listing.token);

        // If fully funded, notify pool to release funds to SME
        if fully_funded {
            let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_contract);
            pool_client.release_funds(&env.current_contract_address(), &invoice_id);
        }

        Ok(())
    }

    /// SME or admin cancels a listing before it is fully funded.
    pub fn cancel_listing(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();
        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != listing.seller && caller != admin {
            return Err(KoraError::Unauthorized);
        }

        listing.is_active = false;
        env.storage().persistent().set(&DataKey::Listing(invoice_id), &listing);
        events::listing_cancelled(&env, invoice_id, &listing.seller);
        Ok(())
    }

    /// Whitelist a stablecoin token for use in listings.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::WhitelistedToken(token), &true);
        Ok(())
    }

    pub fn get_listing(env: Env, invoice_id: u64) -> Result<Listing, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let admin: Address = env
            .storage()
            .instance()
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

    #[test]
    fn test_cancel_listing_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, MarketplaceContract);
        let client = MarketplaceContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let pool = Address::generate(&env);
        let treasury = Address::generate(&env);
        client.initialize(&admin, &nft, &pool, &treasury, &50u32);

        // Listing doesn't exist — should return ListingNotFound
        let stranger = Address::generate(&env);
        let result = client.try_cancel_listing(&stranger, &999u64);
        assert!(result.is_err());
    }
}
