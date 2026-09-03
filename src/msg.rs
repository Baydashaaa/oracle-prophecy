use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;

use crate::state::{Config, Fees, Market, Spec, Status};

#[cw_serde]
pub struct InstantiateMsg {
    pub admin: Option<String>,
    pub resolver: String,
    pub draw_pool: String,
    pub treasury: String,
    pub denom: String,

    pub protocol_bps: u64,
    pub creator_bps: u64,
    pub boost_bps: u64,

    pub creation_bond: Uint128,
    pub promo_fee: Uint128,
    pub min_bet: Uint128,
    pub max_bet: Uint128,

    pub boost_amount: Uint128,
    pub boost_per_week: u64,
    pub challenge_secs: u64,
    pub bet_cutoff_secs: u64,
}

#[cw_serde]
pub enum ExecuteMsg {
    /// Создать рынок. К сообщению прикладывается залог, а при `promoted` -
    /// ещё и плата за продвижение.
    ///
    /// `bets_close_at` контракт не принимает на веру: он обязан быть раньше
    /// `resolve_after` не меньше чем на `bet_cutoff_secs`. Это и есть защита
    /// от ставок в момент, когда исход уже виден.
    Create {
        question: String,
        category: String,
        spec: Spec,
        bets_close_at: u64,
        resolve_after: u64,
        promoted: bool,
    },

    /// Поставить на сторону. Монеты прикладываются к сообщению.
    /// Повторная ставка на ту же сторону увеличивает существующую.
    Bet { market_id: u64, side: bool },

    /// Объявить исход. Только резолвер, только после `resolve_after`.
    /// `reading` - что именно прочитано в цепочке; хранится ради проверки
    /// и в расчёте не участвует.
    Propose {
        market_id: u64,
        outcome: bool,
        reading: String,
    },

    /// Отменить объявленный исход в окне оспаривания. Только админ.
    /// Рынок возвращается в `Locked`, и резолвер объявляет заново.
    Challenge { market_id: u64, reason: String },

    /// Закрыть рынок после окна оспаривания: развести комиссии, вернуть
    /// залог создателю, открыть выплаты. Permissionless - вызвать может
    /// кто угодно, потому что откладывать выплаты не в чьих интересах.
    Settle { market_id: u64 },

    /// Аннулировать рынок и вернуть все ставки. Причина решает судьбу
    /// залога: `bad_spec` - залог сгорает в фонд доплат, остальные - нет.
    Void { market_id: u64, bad_spec: bool, reason: String },

    /// Забрать выигрыш или, у аннулированного рынка, свою ставку обратно.
    Claim { market_id: u64 },

    /// Пополнить фонд доплат. Открыто для всех - спонсировать рынки может
    /// не только казна.
    FundBoost {},

    UpdateConfig {
        admin: Option<String>,
        resolver: Option<String>,
        draw_pool: Option<String>,
        treasury: Option<String>,
        protocol_bps: Option<u64>,
        creator_bps: Option<u64>,
        boost_bps: Option<u64>,
        creation_bond: Option<Uint128>,
        promo_fee: Option<Uint128>,
        min_bet: Option<Uint128>,
        max_bet: Option<Uint128>,
        boost_amount: Option<Uint128>,
        boost_per_week: Option<u64>,
        challenge_secs: Option<u64>,
        bet_cutoff_secs: Option<u64>,
        paused: Option<bool>,
    },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Config)]
    Config {},

    #[returns(Market)]
    Market { market_id: u64 },

    /// Список рынков с фильтром по статусу. Постранично: фронту нужен
    /// список, а не один рынок.
    #[returns(MarketsResponse)]
    Markets {
        status: Option<Status>,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    /// Позиция кошелька на рынке: сколько поставлено с каждой стороны и
    /// сколько причитается, если исход уже известен.
    #[returns(PositionResponse)]
    Position { market_id: u64, address: String },

    #[returns(BoostResponse)]
    Boost {},
}

#[cw_serde]
pub struct MarketsResponse {
    pub markets: Vec<Market>,
}

#[cw_serde]
pub struct PositionResponse {
    pub yes: Uint128,
    pub no: Uint128,
    pub claimed: bool,
    /// Сколько заберёт кошелёк: своя ставка плюс доля банка. Ноль, пока
    /// исход не объявлен.
    pub payout: Uint128,
}

#[cw_serde]
pub struct BoostResponse {
    pub fund: Uint128,
    pub per_market: Uint128,
    pub used_this_week: u64,
    pub per_week: u64,
}

#[cw_serde]
pub struct MigrateMsg {}

/// Доли считаются в базисных пунктах от проигравшего банка.
pub fn bps(amount: Uint128, bps: u64) -> Uint128 {
    amount.multiply_ratio(bps as u128, 10_000u128)
}

/// Копия долей на момент создания: изменение конфига не должно переписывать
/// условия уже открытых рынков.
pub fn fees_from(cfg: &Config) -> Fees {
    Fees {
        protocol_bps: cfg.protocol_bps,
        creator_bps: cfg.creator_bps,
        boost_bps: cfg.boost_bps,
    }
}
