use cosmwasm_std::{StdError, Uint128};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Contract is paused")]
    Paused {},

    #[error("Market {id} not found")]
    NoMarket { id: u64 },

    #[error("Market is not open for bets")]
    NotOpen {},

    #[error("Bets for this market are closed")]
    BetsClosed {},

    #[error("Market is not ready to be resolved yet")]
    TooEarly {},

    #[error("Market is not awaiting settlement")]
    NotProposed {},

    #[error("Challenge window is still open")]
    ChallengeOpen {},

    #[error("Challenge window has passed")]
    ChallengeClosed {},

    #[error("Market is already settled or void")]
    AlreadyClosed {},

    #[error("Send exactly {expected} {denom}")]
    WrongPayment { expected: Uint128, denom: String },

    #[error("Bet must be between {min} and {max}")]
    BetOutOfRange { min: Uint128, max: Uint128 },

    /// Ставки должны закрываться заметно раньше измерения, иначе на рынках
    /// с расчётом по цепочке можно ставить, когда исход уже виден.
    #[error("Bets must close at least {secs}s before the market resolves")]
    CutoffTooShort { secs: u64 },

    #[error("Resolution time must be in the future")]
    ResolveInPast {},

    /// Показатель, который проверяет цепочка, бессмыслен без высоты блока:
    /// «первого сентября» - это сутки, а не момент.
    #[error("A chain metric needs a block height in the spec")]
    SpecNeedsHeight {},

    #[error("A chain metric needs a comparator and a threshold")]
    SpecNeedsThreshold {},

    #[error("Question or criterion is empty")]
    EmptySpec {},

    #[error("Fees add up to {total} bps, which leaves nothing for winners")]
    FeesTooHigh { total: u64 },

    /// Комиссия создателя обязана быть меньше протокольной: иначе ставка на
    /// обе стороны собственного рынка становится прибыльной.
    #[error("Creator fee must stay below the protocol fee")]
    CreatorFeeTooHigh {},

    #[error("Nothing to claim")]
    NothingToClaim {},

    #[error("Already claimed")]
    AlreadyClaimed {},

    #[error("Payouts are not open yet")]
    PayoutsClosed {},
}
