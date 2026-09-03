// Интеграционные тесты oracle-prophecy.
//
// Проверяются не счастливые пути, а границы: попытки поставить после
// закрытия, обойти потолок, забрать выигрыш дважды, накрутить оборот своим
// же рынком, объявить исход раньше срока и чужим кошельком.
//
// Числа взяты маленькими и круглыми намеренно - чтобы ожидаемую выплату
// можно было посчитать в уме и увидеть ошибку глазами, а не отладчиком.

use cosmwasm_std::{coins, Addr, Uint128};
use cw_multi_test::{App, AppBuilder, ContractWrapper, Executor};

use oracle_prophecy::msg::{
    BoostResponse, ExecuteMsg, InstantiateMsg, PositionResponse, QueryMsg,
};
use oracle_prophecy::state::{Config, Market, Spec, Status};

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
const PROMO: u128 = 200;
const CUTOFF: u64 = 86_400;
const CHALLENGE: u64 = 3_600;

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
            protocol_bps: 500,
            creator_bps: 300,
            boost_bps: 200,
            creation_bond: Uint128::new(BOND),
            promo_fee: Uint128::new(PROMO),
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
        metric: Some("oracle_rate".into()),
        param: Some("uusd".into()),
        comparator: Some("gt".into()),
        threshold: Some("0.000060000000000000".into()),
        height: Some(30_312_400),
        criterion: "oracle module exchange rate at the given height".into(),
    }
}

fn now(app: &App) -> u64 {
    // Поле block у App приватное - время читается только геттером.
    app.block_info().time.seconds()
}

fn advance(app: &mut App, secs: u64) {
    app.update_block(|b| {
        b.time = b.time.plus_seconds(secs);
        b.height += secs / 6;
    });
}

fn create(app: &mut App, c: &Addr, promoted: bool) -> u64 {
    let close = now(app) + 1_000;
    let resolve = close + CUTOFF + 1;
    let funds = if promoted { BOND + PROMO } else { BOND };
    app.execute_contract(
        Addr::unchecked(CREATOR),
        c.clone(),
        &ExecuteMsg::Create {
            question: "LUNC above $0.00006 on Monday".into(),
            category: "chain".into(),
            spec: spec(),
            bets_close_at: close,
            resolve_after: resolve,
            promoted,
        },
        &coins(funds, DENOM),
    )
    .unwrap();
    app.wrap()
        .query_wasm_smart::<Market>(c.clone(), &QueryMsg::Market { market_id: 1 })
        .map(|m| m.id)
        .unwrap()
}

fn bet(app: &mut App, c: &Addr, who: &str, side: bool, amount: u128) -> anyhow::Result<()> {
    app.execute_contract(
        Addr::unchecked(who),
        c.clone(),
        &ExecuteMsg::Bet {
            market_id: 1,
            side,
        },
        &coins(amount, DENOM),
    )
    .map(|_| ())
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn balance(app: &App, who: &str) -> u128 {
    app.wrap().query_balance(who, DENOM).unwrap().amount.u128()
}

fn market(app: &App, c: &Addr) -> Market {
    app.wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 1 })
        .unwrap()
}

// ── создание ────────────────────────────────────────────────────────────────

#[test]
fn create_requires_exact_bond() {
    let mut a = app();
    let c = setup(&mut a);
    let close = now(&a) + 1_000;
    let err = a
        .execute_contract(
            Addr::unchecked(CREATOR),
            c.clone(),
            &ExecuteMsg::Create {
                question: "q".into(),
                category: "chain".into(),
                spec: spec(),
                bets_close_at: close,
                resolve_after: close + CUTOFF + 1,
                promoted: false,
            },
            &coins(BOND - 1, DENOM),
        )
        .unwrap_err();
    assert!(err.root_cause().to_string().contains("Send exactly"));
}

#[test]
fn create_enforces_the_cutoff() {
    let mut a = app();
    let c = setup(&mut a);
    let close = now(&a) + 1_000;
    // Ставки закрываются за час до измерения - это и есть та щель, в которую
    // можно поставить, уже зная исход.
    let err = a
        .execute_contract(
            Addr::unchecked(CREATOR),
            c.clone(),
            &ExecuteMsg::Create {
                question: "q".into(),
                category: "chain".into(),
                spec: spec(),
                bets_close_at: close,
                resolve_after: close + 3_600,
                promoted: false,
            },
            &coins(BOND, DENOM),
        )
        .unwrap_err();
    assert!(err.root_cause().to_string().contains("before the market resolves"));
}

#[test]
fn chain_metric_needs_a_height() {
    let mut a = app();
    let c = setup(&mut a);
    let close = now(&a) + 1_000;
    let mut s = spec();
    s.height = None;
    let err = a
        .execute_contract(
            Addr::unchecked(CREATOR),
            c.clone(),
            &ExecuteMsg::Create {
                question: "q".into(),
                category: "chain".into(),
                spec: s,
                bets_close_at: close,
                resolve_after: close + CUTOFF + 1,
                promoted: false,
            },
            &coins(BOND, DENOM),
        )
        .unwrap_err();
    assert!(err.root_cause().to_string().contains("block height"));
}

#[test]
fn promo_fee_goes_straight_to_treasury() {
    let mut a = app();
    let c = setup(&mut a);
    let before = balance(&a, TREASURY);
    create(&mut a, &c, true);
    // Плата за продвижение невозвратна, поэтому не должна лежать на
    // контракте вперемешку с деньгами участников.
    assert_eq!(balance(&a, TREASURY), before + PROMO);
}

// ── ставки ──────────────────────────────────────────────────────────────────

#[test]
fn bets_respect_min_and_cumulative_max() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);

    assert!(bet(&mut a, &c, ALICE, true, 5).is_err(), "меньше минимума");
    assert!(bet(&mut a, &c, ALICE, true, 600).is_ok());
    // Потолок считается по сумме кошелька: 600 + 500 больше 1000, значит
    // обойти его серией ставок нельзя.
    assert!(bet(&mut a, &c, ALICE, true, 500).is_err(), "обход потолка");
    assert!(bet(&mut a, &c, ALICE, true, 400).is_ok());

    let m = market(&a, &c);
    assert_eq!(m.pot_yes, Uint128::new(1_000));
    assert_eq!(m.bettors_yes, 1, "повторная ставка не создаёт участника");
}

#[test]
fn bets_close_on_time() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);
    advance(&mut a, 1_001);
    assert!(bet(&mut a, &c, ALICE, true, 100).is_err());
    // Статус остаётся Open: отказ откатывает транзакцию целиком, и никакая
    // запись при отклонённой ставке не сохраняется. Закрытие определяется
    // временем bets_close_at, и фронт считает его сам.
    assert_eq!(market(&a, &c).status, Status::Open);
}

// ── объявление исхода ───────────────────────────────────────────────────────

#[test]
fn only_resolver_and_only_after_the_time() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);
    bet(&mut a, &c, ALICE, true, 100).unwrap();

    let propose = ExecuteMsg::Propose {
        market_id: 1,
        outcome: true,
        reading: "rate 0.0000612 at height 30312400".into(),
    };
    // рано
    assert!(a
        .execute_contract(Addr::unchecked(RESOLVER), c.clone(), &propose, &[])
        .is_err());

    advance(&mut a, 1_000 + CUTOFF + 2);
    // чужой кошелёк
    assert!(a
        .execute_contract(Addr::unchecked(ALICE), c.clone(), &propose, &[])
        .is_err());
    a.execute_contract(Addr::unchecked(RESOLVER), c.clone(), &propose, &[])
        .unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Proposed);
    assert_eq!(m.outcome, Some(true));
    assert!(m.reading.unwrap().contains("30312400"), "чтение сохранено");
}

#[test]
fn challenge_returns_the_market_for_a_second_reading() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);
    bet(&mut a, &c, ALICE, true, 100).unwrap();
    bet(&mut a, &c, BOB, false, 100).unwrap();
    advance(&mut a, 1_000 + CUTOFF + 2);

    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: true,
            reading: "wrong".into(),
        },
        &[],
    )
    .unwrap();

    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::Challenge {
            market_id: 1,
            reason: "read at the wrong height".into(),
        },
        &[],
    )
    .unwrap();

    // Ошибка резолвера не отменяет рынок - он объявляет заново.
    let m = market(&a, &c);
    assert_eq!(m.status, Status::Locked);
    assert_eq!(m.outcome, None);

    advance(&mut a, CHALLENGE + 1);
    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: false,
            reading: "rate 0.0000501 at height 30312400".into(),
        },
        &[],
    )
    .unwrap();
    assert_eq!(market(&a, &c).outcome, Some(false));
}

// ── расчёт и выплаты ────────────────────────────────────────────────────────

/// Тот же пример, что в описании экономики, только меньше на три нуля:
/// банк 1000, из них 400 на «да» и 600 на «нет», исход «да».
#[test]
fn settle_splits_exactly_as_promised() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);

    bet(&mut a, &c, ALICE, true, 100).unwrap();
    bet(&mut a, &c, BOB, true, 300).unwrap();
    bet(&mut a, &c, CAROL, false, 600).unwrap();

    advance(&mut a, 1_000 + CUTOFF + 2);
    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: true,
            reading: "rate above threshold".into(),
        },
        &[],
    )
    .unwrap();

    // До конца окна оспаривания выплаты закрыты.
    assert!(a
        .execute_contract(
            Addr::unchecked(ALICE),
            c.clone(),
            &ExecuteMsg::Settle { market_id: 1 },
            &[]
        )
        .is_err());

    advance(&mut a, CHALLENGE + 1);
    let (draw0, tre0, cre0) = (balance(&a, DRAW), balance(&a, TREASURY), balance(&a, CREATOR));

    // Settle зовёт посторонний кошелёк - это намеренно permissionless.
    a.execute_contract(
        Addr::unchecked(CAROL),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();

    // 5% от 600 = 30, пополам: 15 в пул розыгрыша, 15 в казну. 3% = 18
    // создателю, плюс возврат залога 50.
    assert_eq!(balance(&a, DRAW), draw0 + 15);
    assert_eq!(balance(&a, TREASURY), tre0 + 15);
    assert_eq!(balance(&a, CREATOR), cre0 + 18 + BOND);
    assert_eq!(market(&a, &c).status, Status::Settled);

    // 2% = 12 остаются на контракте в фонде доплат.
    let boost: BoostResponse = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Boost {})
        .unwrap();
    assert_eq!(boost.fund, Uint128::new(12));

    // Алиса: своя сотня плюс четверть от 540 = 235.
    let pos: PositionResponse = a
        .wrap()
        .query_wasm_smart(
            c.clone(),
            &QueryMsg::Position {
                market_id: 1,
                address: ALICE.into(),
            },
        )
        .unwrap();
    assert_eq!(pos.payout, Uint128::new(235));

    let a0 = balance(&a, ALICE);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(balance(&a, ALICE), a0 + 235);

    // Второй раз забрать нельзя.
    assert!(a
        .execute_contract(
            Addr::unchecked(ALICE),
            c.clone(),
            &ExecuteMsg::Claim { market_id: 1 },
            &[]
        )
        .is_err());

    // Проигравшему забирать нечего.
    assert!(a
        .execute_contract(
            Addr::unchecked(CAROL),
            c.clone(),
            &ExecuteMsg::Claim { market_id: 1 },
            &[]
        )
        .is_err());

    // Боб: 300 из 400 победившей стороны, то есть 300 + 405.
    let b0 = balance(&a, BOB);
    a.execute_contract(
        Addr::unchecked(BOB),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(balance(&a, BOB), b0 + 705);
}

#[test]
fn one_sided_market_refunds_everyone() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);
    bet(&mut a, &c, ALICE, true, 500).unwrap();

    advance(&mut a, 1_000 + CUTOFF + 2);
    a.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: false, // выиграла пустая сторона
            reading: "below threshold".into(),
        },
        &[],
    )
    .unwrap();
    advance(&mut a, CHALLENGE + 1);

    let cre0 = balance(&a, CREATOR);
    a.execute_contract(
        Addr::unchecked(BOB),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();

    // Делить не с кем: рынок аннулирован, залог возвращён - вопрос был
    // нормальный, просто никто не поспорил.
    assert_eq!(market(&a, &c).status, Status::Void);
    assert_eq!(balance(&a, CREATOR), cre0 + BOND);

    let a0 = balance(&a, ALICE);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(balance(&a, ALICE), a0 + 500, "ставка вернулась целиком");
}

#[test]
fn bad_spec_burns_the_bond_into_the_boost_fund() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);
    bet(&mut a, &c, ALICE, true, 100).unwrap();
    bet(&mut a, &c, BOB, false, 100).unwrap();

    let cre0 = balance(&a, CREATOR);
    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::Void {
            market_id: 1,
            bad_spec: true,
            reason: "criterion cannot be checked".into(),
        },
        &[],
    )
    .unwrap();

    assert_eq!(balance(&a, CREATOR), cre0, "залог не вернулся");
    let boost: BoostResponse = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Boost {})
        .unwrap();
    assert_eq!(boost.fund, Uint128::new(BOND));

    // Ставки при этом возвращаются обеим сторонам.
    for who in [ALICE, BOB] {
        let b0 = balance(&a, who);
        a.execute_contract(
            Addr::unchecked(who),
            c.clone(),
            &ExecuteMsg::Claim { market_id: 1 },
            &[],
        )
        .unwrap();
        assert_eq!(balance(&a, who), b0 + 100);
    }
}

// ── защита от накрутки ──────────────────────────────────────────────────────

#[test]
fn self_dealing_stays_unprofitable() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);

    // Создатель ставит на обе стороны своего рынка, надеясь собрать
    // комиссию с оборота.
    bet(&mut a, &c, CREATOR, true, 500).unwrap();
    bet(&mut a, &c, CREATOR, false, 500).unwrap();
    let start = balance(&a, CREATOR);

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
    advance(&mut a, CHALLENGE + 1);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();
    a.execute_contract(
        Addr::unchecked(CREATOR),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .unwrap();

    // Вернулось: залог 50, комиссия создателя 15, выигравшая ставка 500 и
    // 90% проигравшей 450. Против 1000, которые он внёс ставками.
    // Итог по ставкам отрицательный - ровно потому, что доля создателя
    // меньше протокольной.
    let gained = balance(&a, CREATOR) - start;
    assert!(gained < 1_000 + BOND, "накрутка не должна быть прибыльной");
    assert_eq!(gained, BOND + 15 + 500 + 450);
}

#[test]
fn creator_fee_cannot_reach_the_protocol_fee() {
    let mut a = app();
    let c = setup(&mut a);
    // Иначе ставка на обе стороны собственного рынка станет выгодной.
    let err = a
        .execute_contract(
            Addr::unchecked(ADMIN),
            c.clone(),
            &ExecuteMsg::UpdateConfig {
                admin: None,
                resolver: None,
                draw_pool: None,
                treasury: None,
                protocol_bps: None,
                creator_bps: Some(500),
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
            &[],
        )
        .unwrap_err();
    assert!(err
        .root_cause()
        .to_string()
        .contains("below the protocol fee"));
}

// ── доплата казны ───────────────────────────────────────────────────────────

#[test]
fn boost_is_capped_per_week() {
    let mut a = app();
    let c = setup(&mut a);

    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::FundBoost {},
        &coins(1_000, DENOM),
    )
    .unwrap();

    // Квота две штуки в неделю: третий рынок доплаты не получит.
    for i in 0..3 {
        let close = now(&a) + 1_000;
        a.execute_contract(
            Addr::unchecked(CREATOR),
            c.clone(),
            &ExecuteMsg::Create {
                question: format!("market {i}"),
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

    let m1: Market = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 1 })
        .unwrap();
    let m3: Market = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 3 })
        .unwrap();
    assert_eq!(m1.boost, Uint128::new(100));
    assert_eq!(m3.boost, Uint128::zero(), "квота исчерпана");

    // Через неделю счётчик обнуляется.
    advance(&mut a, 604_801);
    let close = now(&a) + 1_000;
    a.execute_contract(
        Addr::unchecked(CREATOR),
        c.clone(),
        &ExecuteMsg::Create {
            question: "next week".into(),
            category: "chain".into(),
            spec: spec(),
            bets_close_at: close,
            resolve_after: close + CUTOFF + 1,
            promoted: false,
        },
        &coins(BOND, DENOM),
    )
    .unwrap();
    let m4: Market = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 4 })
        .unwrap();
    assert_eq!(m4.boost, Uint128::new(100));
}

#[test]
fn boost_joins_the_losing_pot() {
    let mut a = app();
    let c = setup(&mut a);
    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::FundBoost {},
        &coins(1_000, DENOM),
    )
    .unwrap();
    create(&mut a, &c, false);
    assert_eq!(market(&a, &c).boost, Uint128::new(100));

    bet(&mut a, &c, ALICE, true, 100).unwrap();
    bet(&mut a, &c, BOB, false, 100).unwrap();

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
    advance(&mut a, CHALLENGE + 1);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();

    // Делится 100 проигравших плюс 100 доплаты: победителю достаётся своя
    // сотня и 90% от двухсот.
    let a0 = balance(&a, ALICE);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Claim { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(balance(&a, ALICE), a0 + 100 + 180);
}

// ── конфигурация ────────────────────────────────────────────────────────────

#[test]
fn paused_contract_takes_no_money() {
    let mut a = app();
    let c = setup(&mut a);
    create(&mut a, &c, false);

    a.execute_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &ExecuteMsg::UpdateConfig {
            admin: None,
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
            paused: Some(true),
        },
        &[],
    )
    .unwrap();

    assert!(bet(&mut a, &c, ALICE, true, 100).is_err());
    let cfg: Config = a
        .wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Config {})
        .unwrap();
    assert!(cfg.paused);
}
