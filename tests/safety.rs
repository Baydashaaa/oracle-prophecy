// Тесты платёжеспособности и краёв.
//
// Здесь проверяется не логика механики - её закрывает integration.rs, - а то,
// что контракт ни при каких числах не обещает больше, чем держит. Округление
// в CosmWasm идёт вниз, и если хоть одно деление округлится не в ту сторону,
// последний забирающий упрётся в нехватку средств. Такую ошибку невозможно
// заметить на круглых числах, поэтому суммы здесь намеренно некрасивые.

use cosmwasm_std::{coins, Addr, Uint128};
use cw_multi_test::{App, AppBuilder, ContractWrapper, Executor};

use oracle_prophecy::msg::{BoostResponse, ExecuteMsg, InstantiateMsg, QueryMsg};
use oracle_prophecy::state::{Market, Spec, Status};

const DENOM: &str = "uluna";
const ADMIN: &str = "admin";
const RESOLVER: &str = "resolver";
const DRAW: &str = "draw_pool";
const TREASURY: &str = "treasury";
const CREATOR: &str = "creator";
const ALICE: &str = "alice";
const BOB: &str = "bob";
const CAROL: &str = "carol";

const BOND: u128 = 50;
const CUTOFF: u64 = 86_400;
const CHALLENGE: u64 = 3_600;
const PROTOCOL_BPS: u128 = 500;
const CREATOR_BPS: u128 = 300;
const BOOST_BPS: u128 = 200;

fn app() -> App {
    AppBuilder::new().build(|router, _, storage| {
        for who in [ADMIN, CREATOR, ALICE, BOB, CAROL] {
            router
                .bank
                .init_balance(storage, &Addr::unchecked(who), coins(1_000_000, DENOM))
                .unwrap();
        }
    })
}

fn setup(app: &mut App) -> Addr {
    let code = app.store_code(Box::new(ContractWrapper::new(
        oracle_prophecy::contract::execute,
        oracle_prophecy::contract::instantiate,
        oracle_prophecy::contract::query,
    )));
    app.instantiate_contract(
        code,
        Addr::unchecked(ADMIN),
        &InstantiateMsg {
            admin: Some(ADMIN.into()),
            resolver: RESOLVER.into(),
            draw_pool: DRAW.into(),
            treasury: TREASURY.into(),
            denom: DENOM.into(),
            protocol_bps: PROTOCOL_BPS as u64,
            creator_bps: CREATOR_BPS as u64,
            boost_bps: BOOST_BPS as u64,
            creation_bond: Uint128::new(BOND),
            promo_fee: Uint128::new(200),
            min_bet: Uint128::new(10),
            max_bet: Uint128::new(1_000),
            boost_amount: Uint128::new(100),
            boost_per_week: 2,
            challenge_secs: CHALLENGE,
            bet_cutoff_secs: CUTOFF,
        },
        &[],
        "oracle-prophecy",
        Some(ADMIN.into()),
    )
    .unwrap()
}

fn spec() -> Spec {
    Spec {
        metric: Some("total_supply".into()),
        param: None,
        comparator: Some("lt".into()),
        threshold: Some("6000000000000".into()),
        height: Some(30_400_000),
        criterion: "bank supply of uluna at the given height".into(),
    }
}

fn now(app: &App) -> u64 {
    app.block_info().time.seconds()
}

fn advance(app: &mut App, secs: u64) {
    app.update_block(|b| {
        b.time = b.time.plus_seconds(secs);
        b.height += secs / 6;
    });
}

fn create(app: &mut App, c: &Addr) {
    let close = now(app) + 1_000;
    app.execute_contract(
        Addr::unchecked(CREATOR),
        c.clone(),
        &ExecuteMsg::Create {
            question: "supply below 6T".into(),
            category: "chain".into(),
            spec: spec(),
            bets_close_at: close,
            resolve_after: close + CUTOFF + 1,
            promoted: false,
        },
        &coins(BOND, DENOM),
    )
    .unwrap();
}

fn bet(app: &mut App, c: &Addr, who: &str, side: bool, amount: u128) {
    app.execute_contract(
        Addr::unchecked(who),
        c.clone(),
        &ExecuteMsg::Bet {
            market_id: 1,
            side,
        },
        &coins(amount, DENOM),
    )
    .unwrap();
}

fn resolve(app: &mut App, c: &Addr, outcome: bool) {
    advance(app, 1_000 + CUTOFF + 2);
    app.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome,
            reading: "supply 5.4T at height 30400000".into(),
        },
        &[],
    )
    .unwrap();
    advance(app, CHALLENGE + 1);
}

fn settle(app: &mut App, c: &Addr) {
    app.execute_contract(
        Addr::unchecked(BOB),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();
}

fn claim(app: &mut App, c: &Addr, who: &str) -> Result<u128, String> {
    let before = balance(app, who);
    app.execute_contract(
        Addr::unchecked(who),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .map_err(|e| e.root_cause().to_string())?;
    Ok(balance(app, who) - before)
}

fn balance(app: &App, who: &str) -> u128 {
    app.wrap().query_balance(who, DENOM).unwrap().amount.u128()
}

fn market(app: &App, c: &Addr) -> Market {
    app.wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 1 })
        .unwrap()
}

/// Та же формула, что в контракте, включая округление вниз на каждом шаге.
/// Считается независимо от кода контракта - если он разойдётся с описанием
/// экономики хоть на единицу, тест это покажет.
fn expected_payout(mine: u128, winning_pot: u128, losing_pot: u128) -> u128 {
    let kept = 10_000 - PROTOCOL_BPS - CREATOR_BPS - BOOST_BPS;
    let distributable = losing_pot * kept / 10_000;
    mine + distributable * mine / winning_pot
}

// ── платёжеспособность ──────────────────────────────────────────────────────

#[test]
fn awkward_numbers_never_leave_the_contract_short() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);

    // Простые числа: каждое деление даст остаток.
    bet(&mut a, &c, ALICE, true, 11);
    bet(&mut a, &c, BOB, true, 13);
    bet(&mut a, &c, CAROL, true, 17);
    bet(&mut a, &c, ADMIN, false, 97);

    resolve(&mut a, &c, true);
    settle(&mut a, &c);

    let (win, lose) = (41u128, 97u128);
    // Все три забирают, и ни один не должен упереться в нехватку средств -
    // именно это ломается, когда округление уходит не в ту сторону.
    for (who, mine) in [(ALICE, 11u128), (BOB, 13), (CAROL, 17)] {
        let got = claim(&mut a, &c, who).expect("выплата должна пройти");
        assert_eq!(got, expected_payout(mine, win, lose), "{who}");
    }

    // На контракте остаётся только пыль от округления и доля фонда доплат.
    // Отрицательным остаток быть не может - это и есть платёжеспособность.
    let left = balance(&a, c.as_str());
    let boost: BoostResponse = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Boost {})
        .unwrap();
    assert!(left >= boost.fund.u128(), "фонд доплат должен быть покрыт");
    // Округлений вниз ровно семь: три комиссии, делимая часть и три доли
    // победителей. Каждое теряет меньше единицы, значит пыль строго меньше
    // семи. Важно не её значение, а знак: она всегда в пользу контракта.
    assert!(left - boost.fund.u128() < 7, "пыли больше, чем возможно: {left}");
}

#[test]
fn a_single_winner_takes_the_whole_distributable_pot() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);

    bet(&mut a, &c, ALICE, true, 100);
    bet(&mut a, &c, BOB, false, 33);
    bet(&mut a, &c, CAROL, false, 67);

    resolve(&mut a, &c, true);
    settle(&mut a, &c);

    // Один победитель против сотни проигравших: 100 своих плюс 90 из ста.
    assert_eq!(claim(&mut a, &c, ALICE).unwrap(), 190);
    // Проигравшим не достаётся ничего, и повторные попытки тоже отбиваются.
    assert!(claim(&mut a, &c, BOB).is_err());
    assert!(claim(&mut a, &c, CAROL).is_err());
}

// ── края ────────────────────────────────────────────────────────────────────

#[test]
fn a_market_with_no_bets_settles_without_dividing_by_zero() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);

    let cre0 = balance(&a, CREATOR);
    resolve(&mut a, &c, true);
    // Ни одной ставки: деление на размер выигравшей стороны дало бы ноль в
    // знаменателе, поэтому такой рынок обязан уходить в аннулирование.
    settle(&mut a, &c);

    assert_eq!(market(&a, &c).status, Status::Void);
    assert_eq!(balance(&a, CREATOR), cre0 + BOND, "залог вернулся");
}

#[test]
fn one_wallet_on_both_sides_gets_everything_back_when_void() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);

    // Ставить на обе стороны не запрещено - это просто невыгодно.
    bet(&mut a, &c, ALICE, true, 40);
    bet(&mut a, &c, ALICE, false, 60);

    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Void {
            market_id: 1,
            bad_spec: false,
            reason: "metric unavailable at that height".into(),
        },
        &[],
    )
    .unwrap();

    // Возврат обеих ставок одним вызовом, повторный - отказ.
    assert_eq!(claim(&mut a, &c, ALICE).unwrap(), 100);
    assert!(claim(&mut a, &c, ALICE).is_err());
}

#[test]
fn payouts_stay_shut_until_the_challenge_window_closes() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);
    bet(&mut a, &c, ALICE, true, 100);
    bet(&mut a, &c, BOB, false, 100);

    advance(&mut a, 1_000 + CUTOFF + 2);
    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: true,
            reading: "above".into(),
        },
        &[],
    )
    .unwrap();

    // Исход объявлен, но окно оспаривания ещё идёт: деньги трогать рано.
    let err = claim(&mut a, &c, ALICE).unwrap_err();
    assert!(err.contains("not open yet"), "{err}");

    advance(&mut a, CHALLENGE + 1);
    settle(&mut a, &c);
    assert!(claim(&mut a, &c, ALICE).is_ok());
}

#[test]
fn a_settled_market_cannot_be_voided_afterwards() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);
    bet(&mut a, &c, ALICE, true, 100);
    bet(&mut a, &c, BOB, false, 100);
    resolve(&mut a, &c, true);
    settle(&mut a, &c);

    // Иначе админ мог бы отменить рынок после того, как деньги уже разошлись.
    let err = a
        .execute_contract(
            Addr::unchecked(ADMIN),
            c.clone(),
            &ExecuteMsg::Void {
                market_id: 1,
                bad_spec: true,
                reason: "second thoughts".into(),
            },
            &[],
        )
        .unwrap_err();
    assert!(err.root_cause().to_string().contains("already settled"));
}

#[test]
fn strangers_can_neither_void_nor_reconfigure() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c);

    assert!(a
        .execute_contract(
            Addr::unchecked(ALICE),
            c.clone(),
            &ExecuteMsg::Void {
                market_id: 1,
                bad_spec: true,
                reason: "i do not like it".into()
            },
            &[]
        )
        .is_err());

    assert!(a
        .execute_contract(
            Addr::unchecked(ALICE),
            c.clone(),
            &ExecuteMsg::UpdateConfig {
                admin: Some(ALICE.into()),
                resolver: None,
                draw_pool: None,
                treasury: None,
                protocol_bps: None,
                creator_bps: None,
                boost_bps: None,
                creation_bond: None,
                promo_fee: None,
                min_bet: None,
                max_bet: None,
                boost_amount: None,
                boost_per_week: None,
                challenge_secs: None,
                bet_cutoff_secs: None,
                paused: None,
            },
            &[]
        )
        .is_err());
}

#[test]
fn a_voided_market_returns_its_boost_to_the_fund() {
    let mut a = app();
    let c = setup(&mut a);
    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::FundBoost {},
        &coins(500, DENOM),
    )
    .unwrap();
    create(&mut a, &c);
    assert_eq!(market(&a, &c).boost, Uint128::new(100));

    let fund_before: BoostResponse = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Boost {})
        .unwrap();
    assert_eq!(fund_before.fund, Uint128::new(400));

    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::Void {
            market_id: 1,
            bad_spec: false,
            reason: "proposal was withdrawn".into(),
        },
        &[],
    )
    .unwrap();

    // Доплата не сгорает вместе с рынком - она возвращается и уходит
    // следующему.
    let fund_after: BoostResponse = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Boost {})
        .unwrap();
    assert_eq!(fund_after.fund, Uint128::new(500));
}
