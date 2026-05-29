use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum KoraError {
    // Auth & Access
    Unauthorized = 1,
    NotAdmin = 2,
    NotVerifier = 3,
    ProtocolPaused = 4,
    NotPaused = 5,

    // Invoice
    InvoiceNotFound = 10,
    InvoiceAlreadyExists = 11,
    InvalidInvoiceStatus = 12,
    InvoiceExpired = 13,
    InvalidAmount = 14,
    InvalidDueDate = 15,
    InvalidRiskScore = 16,

    // Marketplace
    ListingNotFound = 20,
    ListingAlreadyCancelled = 21,
    ListingExpired = 22,
    FundingDeadlinePassed = 23,
    InsufficientFunds = 24,
    ExceedsFundingTarget = 25,
    AlreadyFullyFunded = 26,

    // Pool
    PoolNotFound = 30,
    PoolAlreadyClosed = 31,
    RepaymentAlreadyMade = 32,
    InsufficientPoolBalance = 33,

    // Treasury
    InvalidFeeRate = 40,
    WithdrawalFailed = 41,
    TokenNotWhitelisted = 42,

    // Risk
    SMENotRegistered = 50,
    DebtorNotRegistered = 51,
    RiskScoreOutOfRange = 52,

    // General
    ArithmeticOverflow = 90,
    /// Subtraction would produce a negative result (underflow).
    ArithmeticUnderflow = 91,
    InvalidAddress = 92,
    EmptyString = 93,
    AlreadyInitialized = 94,
    NotInitialized = 95,
    /// Reentrancy detected — a protected function was called recursively.
    Reentrancy = 96,
}
