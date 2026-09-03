#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    coins, to_json_binary, Addr, BankMsg, Binary, Deps, DepsMut, Env, MessageInfo, Order, Response,
    StdResult, Uint128,
};
use cw_storage_plus::Bound;

use crate::error::ContractError;
use crate::msg::{
    bps, fees_from, BoostResponse, ExecuteMsg, InstantiateMsg, MarketsResponse, MigrateMsg,
    PositionResponse, QueryMsg,
};
use crate::state::{
    side_key, Bet, Config, Market, Spec, Status, BETS, BOOST_FUND, BOOST_WEEK, CONFIG, MARKETS,
    NEXT_ID,
};

const WEEK: u64 = 604_800;
const MAX_LIMIT: u32 = 50;

// ── instantiate ─────────────────────────────────────────────────────────────

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    let cfg = Config {
        admin: msg
            .admin
            .map(|a| deps.api.addr_validate(&a))
            .transpose()?
            .unwrap_or(info.sender),
        resolver: deps.api.addr_validate(&msg.resolver)?,
        draw_pool: deps.api.addr_validate(&msg.draw_pool)?,
        treasury: deps.api.addr_validate(&msg.treasury)?,
        denom: msg.denom,
        protocol_bps: msg.protocol_bps,
        creator_bps: msg.creator_bps,
        boost_bps: msg.boost_bps,
        creation_bond: msg.creation_bond,
        promo_fee: msg.promo_fee,
        min_bet: msg.min_bet,
        max_bet: msg.max_bet,
        boost_amount: msg.boost_amount,
        boost_per_week: msg.boost_per_week,
        challenge_secs: msg.challenge_secs,
        bet_cutoff_secs: msg.bet_cutoff_secs,
        paused: false,
    };
    check_fees(&cfg)?;

    CONFIG.save(deps.storage, &cfg)?;
    NEXT_ID.save(deps.storage, &1u64)?;
    BOOST_FUND.save(deps.storage, &Uint128::zero())?;
    BOOST_WEEK.save(deps.storage, &(0u64, 0u64))?;

    Ok(Response::new().add_attribute("action", "instantiate"))
}

/// Две проверки, которые нельзя обойти правкой конфига.
fn check_fees(cfg: &Config) -> Result<(), ContractError> {
    let total = cfg.protocol_bps + cfg.creator_bps + cfg.boost_bps;
    if total >= 10_000 {
        return Err(ContractError::FeesTooHigh { total });
    }
    // Ставка на обе стороны своего рынка должна оставаться убыточной.
    if cfg.creator_bps >= cfg.protocol_bps {
        return Err(ContractError::CreatorFeeTooHigh {});
    }
    Ok(())
}

// ── execute ─────────────────────────────────────────────────────────────────

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::Create {
            question,
            category,
            spec,
            bets_close_at,
            resolve_after,
            promoted,
        } => exec_create(
            deps,
            env,
            info,
            question,
            category,
            spec,
            bets_close_at,
            resolve_after,
            promoted,
        ),
        ExecuteMsg::Bet { market_id, side } => exec_bet(deps, env, info, market_id, side),
        ExecuteMsg::Propose {
            market_id,
            outcome,
            reading,
        } => exec_propose(deps, env, info, market_id, outcome, reading),
        ExecuteMsg::Challenge { market_id, reason } => {
            exec_challenge(deps, env, info, market_id, reason)
        }
        ExecuteMsg::Settle { market_id } => exec_settle(deps, env, market_id),
        ExecuteMsg::Void {
            market_id,
            bad_spec,
            reason,
        } => exec_void(deps, env, info, market_id, bad_spec, reason),
        ExecuteMsg::Claim { market_id } => exec_claim(deps, env, info, market_id),
        ExecuteMsg::FundBoost {} => exec_fund_boost(deps, info),
        ExecuteMsg::UpdateConfig { .. } => exec_update_config(deps, info, msg),
    }
}

/// Сколько монет нужного деноминала пришло с сообщением.
fn sent(info: &MessageInfo, denom: &str) -> Uint128 {
    info.funds
        .iter()
        .filter(|c| c.denom == denom)
        .map(|c| c.amount)
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn exec_create(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    question: String,
    category: String,
    spec: Spec,
    bets_close_at: u64,
    resolve_after: u64,
    promoted: bool,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    if cfg.paused {
        return Err(ContractError::Paused {});
    }
    if question.trim().is_empty() || spec.criterion.trim().is_empty() {
        return Err(ContractError::EmptySpec {});
    }

    // Показатель, который проверяет цепочка, обязан назвать момент и порог.
    if spec.metric.is_some() {
        if spec.height.is_none() {
            return Err(ContractError::SpecNeedsHeight {});
        }
        let discrete = spec.metric.as_deref() == Some("proposal_passed");
        if !discrete && (spec.comparator.is_none() || spec.threshold.is_none()) {
            return Err(ContractError::SpecNeedsThreshold {});
        }
    }

    let now = env.block.time.seconds();
    if resolve_after <= now {
        return Err(ContractError::ResolveInPast {});
    }
    // Отсечка: между закрытием ставок и измерением должен быть зазор.
    if bets_close_at + cfg.bet_cutoff_secs > resolve_after || bets_close_at <= now {
        return Err(ContractError::CutoffTooShort {
            secs: cfg.bet_cutoff_secs,
        });
    }

    let expected = cfg.creation_bond + if promoted { cfg.promo_fee } else { Uint128::zero() };
    if sent(&info, &cfg.denom) != expected {
        return Err(ContractError::WrongPayment {
            expected,
            denom: cfg.denom,
        });
    }

    // Доплата казны: только пока в фонде есть деньги и не выбрана недельная
    // квота. Расход ограничен заранее и не зависит от числа рынков.
    let mut fund = BOOST_FUND.load(deps.storage)?;
    let (week_start, used) = BOOST_WEEK.load(deps.storage)?;
    let (week_start, used) = if now >= week_start + WEEK {
        (now, 0u64)
    } else {
        (week_start, used)
    };
    let boost = if used < cfg.boost_per_week && fund >= cfg.boost_amount && promoted_or_any(promoted)
    {
        fund -= cfg.boost_amount;
        BOOST_WEEK.save(deps.storage, &(week_start, used + 1))?;
        cfg.boost_amount
    } else {
        BOOST_WEEK.save(deps.storage, &(week_start, used))?;
        Uint128::zero()
    };
    BOOST_FUND.save(deps.storage, &fund)?;

    let id = NEXT_ID.load(deps.storage)?;
    NEXT_ID.save(deps.storage, &(id + 1))?;

    let market = Market {
        id,
        creator: info.sender.clone(),
        question,
        category,
        spec,
        fees: fees_from(&cfg),
        bets_close_at,
        resolve_after,
        status: Status::Open,
        outcome: None,
        reading: None,
        proposed_at: None,
        pot_yes: Uint128::zero(),
        pot_no: Uint128::zero(),
        boost,
        bettors_yes: 0,
        bettors_no: 0,
        bond: cfg.creation_bond,
        bond_returned: false,
        promoted,
    };
    MARKETS.save(deps.storage, id, &market)?;

    // Плата за продвижение невозвратна и уходит сразу: держать её на
    // контракте значило бы смешивать её с деньгами участников.
    let mut res = Response::new()
        .add_attribute("action", "create")
        .add_attribute("market_id", id.to_string())
        .add_attribute("boost", boost);
    if promoted && !cfg.promo_fee.is_zero() {
        res = res.add_message(BankMsg::Send {
            to_address: cfg.treasury.to_string(),
            amount: coins(cfg.promo_fee.u128(), &cfg.denom),
        });
    }
    Ok(res)
}

/// Доплату получают все рынки, не только продвигаемые. Отдельная функция -
/// чтобы правило было видно, а не растворялось в условии.
fn promoted_or_any(_promoted: bool) -> bool {
    true
}

fn exec_bet(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    market_id: u64,
    side: bool,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    if cfg.paused {
        return Err(ContractError::Paused {});
    }
    let mut m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;
    if m.status != Status::Open {
        return Err(ContractError::NotOpen {});
    }
    // Ошибка откатывает всю транзакцию, поэтому менять здесь статус
    // бессмысленно: запись всё равно не сохранится. Закрытие определяется
    // временем, а не полем - так одно состояние, а не два.
    if env.block.time.seconds() >= m.bets_close_at {
        return Err(ContractError::BetsClosed {});
    }

    let amount = sent(&info, &cfg.denom);
    let key = (market_id, side_key(side), &info.sender);
    let existing = BETS.may_load(deps.storage, key)?;
    let total = existing.as_ref().map(|b| b.amount).unwrap_or_default() + amount;
    // Потолок считается по сумме, а не по одной ставке: иначе его обходят
    // десятью подряд.
    if amount < cfg.min_bet || total > cfg.max_bet {
        return Err(ContractError::BetOutOfRange {
            min: cfg.min_bet,
            max: cfg.max_bet,
        });
    }

    if existing.is_none() {
        if side {
            m.bettors_yes += 1;
        } else {
            m.bettors_no += 1;
        }
    }
    if side {
        m.pot_yes += amount;
    } else {
        m.pot_no += amount;
    }

    BETS.save(
        deps.storage,
        key,
        &Bet {
            amount: total,
            claimed: false,
        },
    )?;
    MARKETS.save(deps.storage, market_id, &m)?;

    Ok(Response::new()
        .add_attribute("action", "bet")
        .add_attribute("market_id", market_id.to_string())
        .add_attribute("side", if side { "yes" } else { "no" })
        .add_attribute("amount", amount))
}

fn exec_propose(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    market_id: u64,
    outcome: bool,
    reading: String,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    if info.sender != cfg.resolver {
        return Err(ContractError::Unauthorized {});
    }
    let mut m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;
    if !matches!(m.status, Status::Open | Status::Locked) {
        return Err(ContractError::AlreadyClosed {});
    }
    let now = env.block.time.seconds();
    if now < m.resolve_after {
        return Err(ContractError::TooEarly {});
    }

    m.status = Status::Proposed;
    m.outcome = Some(outcome);
    m.reading = Some(reading.clone());
    m.proposed_at = Some(now);
    MARKETS.save(deps.storage, market_id, &m)?;

    Ok(Response::new()
        .add_attribute("action", "propose")
        .add_attribute("market_id", market_id.to_string())
        .add_attribute("outcome", outcome.to_string())
        .add_attribute("reading", reading))
}

fn exec_challenge(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    market_id: u64,
    reason: String,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    if info.sender != cfg.admin {
        return Err(ContractError::Unauthorized {});
    }
    let mut m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;
    if m.status != Status::Proposed {
        return Err(ContractError::NotProposed {});
    }
    if env.block.time.seconds() >= m.proposed_at.unwrap_or_default() + cfg.challenge_secs {
        return Err(ContractError::ChallengeClosed {});
    }

    // Возврат в Locked, а не аннулирование: ошибка резолвера не должна
    // отменять рынок, он просто объявляет заново.
    m.status = Status::Locked;
    m.outcome = None;
    m.reading = None;
    m.proposed_at = None;
    MARKETS.save(deps.storage, market_id, &m)?;

    Ok(Response::new()
        .add_attribute("action", "challenge")
        .add_attribute("market_id", market_id.to_string())
        .add_attribute("reason", reason))
}

fn exec_settle(deps: DepsMut, env: Env, market_id: u64) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    let mut m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;
    if m.status != Status::Proposed {
        return Err(ContractError::NotProposed {});
    }
    if env.block.time.seconds() < m.proposed_at.unwrap_or_default() + cfg.challenge_secs {
        return Err(ContractError::ChallengeOpen {});
    }

    // Односторонний рынок делить не между кем. Возвращаем всё: доплата
    // казны уходит обратно в фонд, залог создателя возвращается - вопрос
    // был нормальный, просто никто не поспорил.
    if m.winning_pot().is_zero() {
        return void_market(deps, m, false, "no winning side".to_string());
    }

    let losing = m.losing_pot();
    let protocol = bps(losing, m.fees.protocol_bps);
    let creator = bps(losing, m.fees.creator_bps);
    let boost_cut = bps(losing, m.fees.boost_bps);

    // Половина протокольной доли идёт в призовой фонд розыгрыша: раздел
    // вопросов кормит лотерею, а не живёт рядом с ней.
    let to_draw = protocol.multiply_ratio(1u128, 2u128);
    let to_treasury = protocol - to_draw;

    let mut fund = BOOST_FUND.load(deps.storage)?;
    fund += boost_cut;
    BOOST_FUND.save(deps.storage, &fund)?;

    let mut msgs: Vec<BankMsg> = vec![];
    if !to_draw.is_zero() {
        msgs.push(BankMsg::Send {
            to_address: cfg.draw_pool.to_string(),
            amount: coins(to_draw.u128(), &cfg.denom),
        });
    }
    if !to_treasury.is_zero() {
        msgs.push(BankMsg::Send {
            to_address: cfg.treasury.to_string(),
            amount: coins(to_treasury.u128(), &cfg.denom),
        });
    }
    if !creator.is_zero() {
        msgs.push(BankMsg::Send {
            to_address: m.creator.to_string(),
            amount: coins(creator.u128(), &cfg.denom),
        });
    }
    // Залог возвращается: рынок дошёл до расчёта, формулировка оказалась
    // рабочей.
    if !m.bond.is_zero() && !m.bond_returned {
        msgs.push(BankMsg::Send {
            to_address: m.creator.to_string(),
            amount: coins(m.bond.u128(), &cfg.denom),
        });
        m.bond_returned = true;
    }

    m.status = Status::Settled;
    MARKETS.save(deps.storage, market_id, &m)?;

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "settle")
        .add_attribute("market_id", market_id.to_string())
        .add_attribute("losing_pot", losing)
        .add_attribute("to_draw", to_draw)
        .add_attribute("to_treasury", to_treasury)
        .add_attribute("to_creator", creator))
}

fn exec_void(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    market_id: u64,
    bad_spec: bool,
    reason: String,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    if info.sender != cfg.admin && info.sender != cfg.resolver {
        return Err(ContractError::Unauthorized {});
    }
    let m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;
    if matches!(m.status, Status::Settled | Status::Void) {
        return Err(ContractError::AlreadyClosed {});
    }
    void_market(deps, m, bad_spec, reason)
}

/// Общий путь аннулирования: ставки остаются на контракте и разбираются
/// через Claim, доплата возвращается в фонд, а судьбу залога решает причина.
fn void_market(
    deps: DepsMut,
    mut m: Market,
    bad_spec: bool,
    reason: String,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;

    let mut fund = BOOST_FUND.load(deps.storage)?;
    fund += m.boost;
    // Залог сгорает только за непроверяемую формулировку. Отказ узла или
    // отсутствие второй стороны - не вина создателя.
    if bad_spec {
        fund += m.bond;
        m.bond_returned = true;
    }
    BOOST_FUND.save(deps.storage, &fund)?;
    m.boost = Uint128::zero();

    let mut msgs: Vec<BankMsg> = vec![];
    if !bad_spec && !m.bond.is_zero() && !m.bond_returned {
        msgs.push(BankMsg::Send {
            to_address: m.creator.to_string(),
            amount: coins(m.bond.u128(), &cfg.denom),
        });
        m.bond_returned = true;
    }

    m.status = Status::Void;
    let id = m.id;
    MARKETS.save(deps.storage, id, &m)?;

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "void")
        .add_attribute("market_id", id.to_string())
        .add_attribute("bad_spec", bad_spec.to_string())
        .add_attribute("reason", reason))
}

fn exec_claim(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    market_id: u64,
) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    let m = MARKETS
        .may_load(deps.storage, market_id)?
        .ok_or(ContractError::NoMarket { id: market_id })?;

    let payout = match m.status {
        Status::Void => {
            // Возврат: забираем свои ставки с обеих сторон.
            let mut total = Uint128::zero();
            for side in [true, false] {
                let key = (market_id, side_key(side), &info.sender);
                if let Some(mut b) = BETS.may_load(deps.storage, key)? {
                    if !b.claimed {
                        total += b.amount;
                        b.claimed = true;
                        BETS.save(deps.storage, key, &b)?;
                    }
                }
            }
            total
        }
        Status::Settled => {
            let win = m.outcome.unwrap_or(false);
            let key = (market_id, side_key(win), &info.sender);
            let mut b = BETS
                .may_load(deps.storage, key)?
                .ok_or(ContractError::NothingToClaim {})?;
            if b.claimed {
                return Err(ContractError::AlreadyClaimed {});
            }
            // Проигравшая ставка остаётся на контракте и уже разделена -
            // помечаем её как забранную, чтобы Claim не звали повторно.
            let lose_key = (market_id, side_key(!win), &info.sender);
            if let Some(mut lb) = BETS.may_load(deps.storage, lose_key)? {
                lb.claimed = true;
                BETS.save(deps.storage, lose_key, &lb)?;
            }

            let losing = m.losing_pot();
            let kept = 10_000 - m.fees.protocol_bps - m.fees.creator_bps - m.fees.boost_bps;
            let share = bps(losing, kept).multiply_ratio(b.amount, m.winning_pot());

            b.claimed = true;
            BETS.save(deps.storage, key, &b)?;
            b.amount + share
        }
        _ => return Err(ContractError::PayoutsClosed {}),
    };

    if payout.is_zero() {
        return Err(ContractError::NothingToClaim {});
    }

    Ok(Response::new()
        .add_message(BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: coins(payout.u128(), &cfg.denom),
        })
        .add_attribute("action", "claim")
        .add_attribute("market_id", market_id.to_string())
        .add_attribute("payout", payout))
}

fn exec_fund_boost(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let cfg = CONFIG.load(deps.storage)?;
    let amount = sent(&info, &cfg.denom);
    if amount.is_zero() {
        return Err(ContractError::WrongPayment {
            expected: Uint128::one(),
            denom: cfg.denom,
        });
    }
    let mut fund = BOOST_FUND.load(deps.storage)?;
    fund += amount;
    BOOST_FUND.save(deps.storage, &fund)?;

    Ok(Response::new()
        .add_attribute("action", "fund_boost")
        .add_attribute("amount", amount))
}

fn exec_update_config(
    deps: DepsMut,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    let mut cfg = CONFIG.load(deps.storage)?;
    if info.sender != cfg.admin {
        return Err(ContractError::Unauthorized {});
    }
    if let ExecuteMsg::UpdateConfig {
        admin,
        resolver,
        draw_pool,
        treasury,
        protocol_bps,
        creator_bps,
        boost_bps,
        creation_bond,
        promo_fee,
        min_bet,
        max_bet,
        boost_amount,
        boost_per_week,
        challenge_secs,
        bet_cutoff_secs,
        paused,
    } = msg
    {
        if let Some(v) = admin {
            cfg.admin = deps.api.addr_validate(&v)?;
        }
        if let Some(v) = resolver {
            cfg.resolver = deps.api.addr_validate(&v)?;
        }
        if let Some(v) = draw_pool {
            cfg.draw_pool = deps.api.addr_validate(&v)?;
        }
        if let Some(v) = treasury {
            cfg.treasury = deps.api.addr_validate(&v)?;
        }
        if let Some(v) = protocol_bps {
            cfg.protocol_bps = v;
        }
        if let Some(v) = creator_bps {
            cfg.creator_bps = v;
        }
        if let Some(v) = boost_bps {
            cfg.boost_bps = v;
        }
        if let Some(v) = creation_bond {
            cfg.creation_bond = v;
        }
        if let Some(v) = promo_fee {
            cfg.promo_fee = v;
        }
        if let Some(v) = min_bet {
            cfg.min_bet = v;
        }
        if let Some(v) = max_bet {
            cfg.max_bet = v;
        }
        if let Some(v) = boost_amount {
            cfg.boost_amount = v;
        }
        if let Some(v) = boost_per_week {
            cfg.boost_per_week = v;
        }
        if let Some(v) = challenge_secs {
            cfg.challenge_secs = v;
        }
        if let Some(v) = bet_cutoff_secs {
            cfg.bet_cutoff_secs = v;
        }
        if let Some(v) = paused {
            cfg.paused = v;
        }
    }
    // Проверка та же, что при создании: правкой конфига её не обойти.
    check_fees(&cfg)?;
    CONFIG.save(deps.storage, &cfg)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

// ── query ───────────────────────────────────────────────────────────────────

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::Market { market_id } => to_json_binary(&MARKETS.load(deps.storage, market_id)?),
        QueryMsg::Markets {
            status,
            start_after,
            limit,
        } => to_json_binary(&query_markets(deps, status, start_after, limit)?),
        QueryMsg::Position { market_id, address } => {
            to_json_binary(&query_position(deps, market_id, address)?)
        }
        QueryMsg::Boost {} => {
            let cfg = CONFIG.load(deps.storage)?;
            let (_, used) = BOOST_WEEK.load(deps.storage)?;
            to_json_binary(&BoostResponse {
                fund: BOOST_FUND.load(deps.storage)?,
                per_market: cfg.boost_amount,
                used_this_week: used,
                per_week: cfg.boost_per_week,
            })
        }
    }
}

fn query_markets(
    deps: Deps,
    status: Option<Status>,
    start_after: Option<u64>,
    limit: Option<u32>,
) -> StdResult<MarketsResponse> {
    let limit = limit.unwrap_or(20).min(MAX_LIMIT) as usize;
    let start = start_after.map(Bound::exclusive);
    let markets = MARKETS
        .range(deps.storage, start, None, Order::Ascending)
        .filter_map(|item| item.ok().map(|(_, m)| m))
        .filter(|m| status.as_ref().map(|s| &m.status == s).unwrap_or(true))
        .take(limit)
        .collect();
    Ok(MarketsResponse { markets })
}

fn query_position(deps: Deps, market_id: u64, address: String) -> StdResult<PositionResponse> {
    let addr: Addr = deps.api.addr_validate(&address)?;
    let m = MARKETS.load(deps.storage, market_id)?;

    let yes = BETS
        .may_load(deps.storage, (market_id, side_key(true), &addr))?
        .unwrap_or(Bet {
            amount: Uint128::zero(),
            claimed: false,
        });
    let no = BETS
        .may_load(deps.storage, (market_id, side_key(false), &addr))?
        .unwrap_or(Bet {
            amount: Uint128::zero(),
            claimed: false,
        });

    let payout = match m.status {
        Status::Void => yes.amount + no.amount,
        Status::Proposed | Status::Settled => {
            let win = m.outcome.unwrap_or(false);
            let mine = if win { yes.amount } else { no.amount };
            if mine.is_zero() || m.winning_pot().is_zero() {
                Uint128::zero()
            } else {
                let kept = 10_000 - m.fees.protocol_bps - m.fees.creator_bps - m.fees.boost_bps;
                mine + bps(m.losing_pot(), kept).multiply_ratio(mine, m.winning_pot())
            }
        }
        _ => Uint128::zero(),
    };

    Ok(PositionResponse {
        yes: yes.amount,
        no: no.amount,
        claimed: yes.claimed || no.claimed,
        payout,
    })
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(_deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    Ok(Response::new().add_attribute("action", "migrate"))
}
