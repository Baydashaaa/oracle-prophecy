// Споры, арбитр и таймауты - миграция 0.2.0.
//
// Проверяется не только логика состояний, но и деньги: кто сколько получил
// в каждом исходе спора. Ошибка в распределении залога видна только на
// сверке балансов, поэтому каждый исход сведён до копейки.
//
// Пример везде один и тот же, чтобы числа можно было проверить в уме:
// Alice 400 на YES, Bob 600 на NO, доплаты нет. Комиссии 5% / 3% / 2%
// берутся с проигравшего банка, победителям остаётся 90% от него.

use cosmwasm_std::{coins, from_json, Addr, Uint128};
use cw_multi_test::{App, AppBuilder, ContractWrapper, Executor};

use oracle_prophecy::msg::{BoostResponse, ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};
use oracle_prophecy::state::{Config, Market, Spec, Status};

const DENOM: &str = "uluna";
const ADMIN: &str = "admin";
const RESOLVER: &str = "resolver";
const ARBITER: &str = "arbiter";
const DRAW: &str = "draw_pool";
const TREASURY: &str = "treasury";
const CREATOR: &str = "creator";
const ALICE: &str = "alice";
const BOB: &str = "bob";
const CAROL: &str = "carol";

const START: u128 = 1_000_000;
const BOND: u128 = 50;
const CH_BOND: u128 = 50;
const CUTOFF: u64 = 86_400;
const CHALLENGE: u64 = 3_600;
const ARBITER_SECS: u64 = 7_200;
const GRACE: u64 = 604_800;

fn app() -> App {
    AppBuilder::new().build(|router, _, storage| {
        for who in [ADMIN, CREATOR, ALICE, BOB, CAROL, RESOLVER, ARBITER] {
            router
                .bank
                .init_balance(storage, &Addr::unchecked(who), coins(START, DENOM))
                .unwrap();
        }
    })
}

fn code(app: &mut App) -> u64 {
    app.store_code(Box::new(
        ContractWrapper::new(
            oracle_prophecy::contract::execute,
            oracle_prophecy::contract::instantiate,
            oracle_prophecy::contract::query,
        )
        .with_migrate(oracle_prophecy::contract::migrate),
    ))
}

fn setup(app: &mut App) -> Addr {
    let id = code(app);
    app.instantiate_contract(
        id,
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
            promo_fee: Uint128::new(200),
            min_bet: Uint128::new(10),
            max_bet: Uint128::new(1_000),
            boost_amount: Uint128::new(100),
            boost_per_week: 2,
            challenge_secs: CHALLENGE,
            bet_cutoff_secs: CUTOFF,
            arbiter: Some(ARBITER.into()),
            challenge_bond: Uint128::new(CH_BOND),
            arbiter_secs: ARBITER_SECS,
            resolve_grace_secs: GRACE,
        },
        &[],
        "oracle-prophecy",
        Some(ADMIN.into()),
    )
    .unwrap()
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

fn balance(app: &App, who: &str) -> u128 {
    app.wrap().query_balance(who, DENOM).unwrap().amount.u128()
}

fn market(app: &App, c: &Addr) -> Market {
    app.wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 1 })
        .unwrap()
}

/// После всех выплат на контракте должен лежать ровно фонд доплат. Больше -
/// деньги застряли, меньше - кто-то получил чужое.
fn assert_solvent(app: &App, c: &Addr) {
    assert_eq!(balance(app, c.as_str()), boost_fund(app, c), "контракт разошёлся с фондом");
}

fn boost_fund(app: &App, c: &Addr) -> u128 {
    app.wrap()
        .query_wasm_smart::<BoostResponse>(c.clone(), &QueryMsg::Boost {})
        .unwrap()
        .fund
        .u128()
}

fn exec(app: &mut App, c: &Addr, who: &str, msg: &ExecuteMsg, funds: u128) -> anyhow::Result<()> {
    let f = if funds == 0 { vec![] } else { coins(funds, DENOM) };
    app.execute_contract(Addr::unchecked(who), c.clone(), msg, &f)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!(e.root_cause().to_string()))
}

/// Рынок, две ставки, срок наступил: готов к объявлению исхода.
fn ready_market(app: &mut App, c: &Addr) {
    let close = now(app) + 1_000;
    exec(
        app,
        c,
        CREATOR,
        &ExecuteMsg::Create {
            question: "Will LUNC supply be below 6T?".into(),
            category: "economy".into(),
            spec: Spec {
                metric: Some("total_supply".into()),
                param: None,
                comparator: Some("lt".into()),
                threshold: Some("6000000000000000000".into()),
                height: Some(30_312_400),
                criterion: "bank supply of uluna at the given height".into(),
                unit: Some("uluna".into()),
            },
            bets_close_at: close,
            resolve_after: close + CUTOFF + 1,
            promoted: false,
        },
        BOND,
    )
    .unwrap();
    exec(app, c, ALICE, &ExecuteMsg::Predict { market_id: 1, side: true }, 400).unwrap();
    exec(app, c, BOB, &ExecuteMsg::Predict { market_id: 1, side: false }, 600).unwrap();
    advance(app, 1_000 + CUTOFF + 2);
}

fn propose(app: &mut App, c: &Addr, outcome: bool) {
    exec(
        app,
        c,
        RESOLVER,
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome,
            reading: "supply 6.45T LUNC at height 30312400".into(),
        },
        0,
    )
    .unwrap();
}

fn challenge(app: &mut App, c: &Addr, who: &str) -> anyhow::Result<()> {
    exec(
        app,
        c,
        who,
        &ExecuteMsg::Challenge {
            market_id: 1,
            reading: "the resolver read the wrong height".into(),
        },
        CH_BOND,
    )
}

fn rule(app: &mut App, c: &Addr, who: &str, outcome: Option<bool>) -> anyhow::Result<()> {
    exec(
        app,
        c,
        who,
        &ExecuteMsg::Rule {
            market_id: 1,
            outcome,
            bad_spec: false,
            ruling: "re-read at height 30312400".into(),
        },
        0,
    )
}

fn claim(app: &mut App, c: &Addr, who: &str) -> anyhow::Result<()> {
    exec(app, c, who, &ExecuteMsg::Claim { market_id: 1 }, 0)
}

fn expire(app: &mut App, c: &Addr, who: &str) -> anyhow::Result<()> {
    exec(app, c, who, &ExecuteMsg::Expire { market_id: 1 }, 0)
}

// ── кто может оспорить ──────────────────────────────────────────────────────

#[test]
fn anyone_can_challenge_and_the_market_waits_for_the_arbiter() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);

    challenge(&mut a, &c, CAROL).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Disputed);
    assert_eq!(m.challenger, Some(Addr::unchecked(CAROL)));
    assert_eq!(m.challenge_bond, Uint128::new(CH_BOND));
    // Объявление резолвера не стирается: арбитру и всем остальным нужно
    // видеть, кто что утверждал.
    assert_eq!(m.outcome, Some(true));
    assert!(m.reading.is_some());
    assert_eq!(balance(&a, CAROL), START - CH_BOND);
}

#[test]
fn resolver_and_arbiter_cannot_challenge() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);

    let e = challenge(&mut a, &c, RESOLVER).unwrap_err().to_string();
    assert!(e.contains("cannot challenge"), "{e}");
    let e = challenge(&mut a, &c, ARBITER).unwrap_err().to_string();
    assert!(e.contains("cannot challenge"), "{e}");
}

#[test]
fn challenge_needs_the_exact_bond() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);

    let e = exec(
        &mut a,
        &c,
        CAROL,
        &ExecuteMsg::Challenge {
            market_id: 1,
            reading: "cheap".into(),
        },
        CH_BOND - 1,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("Send exactly"), "{e}");
}

#[test]
fn challenge_is_refused_after_the_window() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    advance(&mut a, CHALLENGE);

    let e = challenge(&mut a, &c, CAROL).unwrap_err().to_string();
    assert!(e.contains("Challenge window has passed"), "{e}");
}

#[test]
fn nothing_pays_out_during_a_dispute() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();
    advance(&mut a, CHALLENGE + 1);

    // Окно оспаривания прошло, но спор не решён: закрыть рынок нельзя.
    let e = exec(&mut a, &c, ALICE, &ExecuteMsg::Settle { market_id: 1 }, 0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not awaiting settlement"), "{e}");
    let e = claim(&mut a, &c, ALICE).unwrap_err().to_string();
    assert!(e.contains("Payouts are not open"), "{e}");
}

// ── исходы спора, с деньгами ────────────────────────────────────────────────

/// Резолвер объявил YES, Carol оспорила, арбитр решил NO.
/// Проигравший банк 400 (YES): протокол 20, создатель 12, доплаты 8,
/// победителям 360. Протокольные 20 уходят Carol вместе с её залогом,
/// розыгрыш и казна не получают ничего.
#[test]
fn challenger_who_was_right_takes_the_protocol_share() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();

    rule(&mut a, &c, ARBITER, Some(false)).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Settled);
    assert_eq!(m.outcome, Some(false));
    assert!(m.ruling.is_some());
    assert_eq!(m.challenge_bond, Uint128::zero());

    assert_eq!(balance(&a, CAROL), START - CH_BOND + CH_BOND + 20);
    assert_eq!(balance(&a, DRAW), 0);
    assert_eq!(balance(&a, TREASURY), 0);
    assert_eq!(balance(&a, CREATOR), START - BOND + BOND + 12);

    claim(&mut a, &c, BOB).unwrap();
    assert_eq!(balance(&a, BOB), START - 600 + 600 + 360);
    // Alice поставила на объявленную, но неверную сторону - ей ничего.
    assert!(claim(&mut a, &c, ALICE).is_err());
    assert_solvent(&a, &c);
}

/// Резолвер объявил NO верно, Carol оспорила зря. Её залог уходит в фонд
/// доплат, рынок рассчитывается как обычно: розыгрыш и казна по 10.
#[test]
fn challenger_who_was_wrong_loses_the_bond_to_the_boost_fund() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, false);
    challenge(&mut a, &c, CAROL).unwrap();

    rule(&mut a, &c, ARBITER, Some(false)).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Settled);
    assert_eq!(m.outcome, Some(false));

    assert_eq!(balance(&a, CAROL), START - CH_BOND);
    // Залог ушёл не резолверу: у того не должно быть мотива нарываться
    // на споры.
    assert_eq!(balance(&a, RESOLVER), START);
    assert_eq!(boost_fund(&a, &c), CH_BOND + 8);
    assert_eq!(balance(&a, DRAW), 10);
    assert_eq!(balance(&a, TREASURY), 10);

    claim(&mut a, &c, BOB).unwrap();
    assert_eq!(balance(&a, BOB), START + 360);
    assert_solvent(&a, &c);
}

/// Арбитр не может установить исход: рынок аннулируется, всё возвращается,
/// включая залог оспорившего, и причина записана в самом рынке.
#[test]
fn arbiter_void_returns_every_stake_and_the_challenge_bond() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();

    rule(&mut a, &c, ARBITER, None).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Void);
    assert_eq!(m.void_reason.as_deref(), Some("voided by the arbiter"));
    assert!(!m.bad_spec);
    assert_eq!(balance(&a, CAROL), START);
    assert_eq!(balance(&a, CREATOR), START);

    claim(&mut a, &c, ALICE).unwrap();
    claim(&mut a, &c, BOB).unwrap();
    assert_eq!(balance(&a, ALICE), START);
    assert_eq!(balance(&a, BOB), START);
    assert_solvent(&a, &c);
}

// ── окончательность и полномочия ────────────────────────────────────────────

#[test]
fn only_the_arbiter_rules() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();

    for who in [ADMIN, RESOLVER, CAROL] {
        let e = rule(&mut a, &c, who, Some(false)).unwrap_err().to_string();
        assert!(e.contains("Unauthorized"), "{who}: {e}");
    }
}

/// Решение окончательное: ни второго спора, ни второго решения.
#[test]
fn a_ruling_is_final() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();
    rule(&mut a, &c, ARBITER, Some(false)).unwrap();

    let e = challenge(&mut a, &c, ALICE).unwrap_err().to_string();
    assert!(e.contains("not awaiting settlement"), "{e}");
    let e = rule(&mut a, &c, ARBITER, Some(true)).unwrap_err().to_string();
    assert!(e.contains("not under dispute"), "{e}");
}

// ── таймауты ────────────────────────────────────────────────────────────────

/// Арбитр молчит - рынок аннулируется, а не остаётся с оспоренным исходом.
/// Иначе неудобный спор можно было бы просто не заметить.
#[test]
fn a_silent_arbiter_leads_to_void_not_to_the_proposed_outcome() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, true);
    challenge(&mut a, &c, CAROL).unwrap();

    advance(&mut a, ARBITER_SECS - 1);
    let e = expire(&mut a, &c, ALICE).unwrap_err().to_string();
    assert!(e.contains("cannot be expired yet"), "{e}");

    advance(&mut a, 1);
    // Опоздавшее решение не принимается - арбитр не перехватит void.
    let e = rule(&mut a, &c, ARBITER, Some(true)).unwrap_err().to_string();
    assert!(e.contains("time to rule has passed"), "{e}");

    expire(&mut a, &c, ALICE).unwrap();
    let m = market(&a, &c);
    assert_eq!(m.status, Status::Void);
    assert_eq!(m.void_reason.as_deref(), Some("the arbiter did not rule in time"));
    assert_eq!(balance(&a, CAROL), START);
}

/// Резолвер пропал. Через неделю после срока рынок может аннулировать кто
/// угодно - худший исход при потерянном ключе это возврат, а не заморозка.
#[test]
fn a_silent_resolver_lets_anyone_void_after_the_grace_period() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);

    advance(&mut a, GRACE - 10);
    let e = expire(&mut a, &c, CAROL).unwrap_err().to_string();
    assert!(e.contains("cannot be expired yet"), "{e}");

    advance(&mut a, 10);
    expire(&mut a, &c, CAROL).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.status, Status::Void);
    assert!(m.void_reason.unwrap().contains("grace period"));
    // Создатель не виноват, что резолвер молчит.
    assert!(!m.bad_spec);
    assert_eq!(balance(&a, CREATOR), START);

    claim(&mut a, &c, ALICE).unwrap();
    claim(&mut a, &c, BOB).unwrap();
    assert_eq!(balance(&a, ALICE), START);
    assert_eq!(balance(&a, BOB), START);
    assert_solvent(&a, &c);
}

#[test]
fn a_settled_market_cannot_be_expired() {
    let mut a = app();
    let c = setup(&mut a);
    ready_market(&mut a, &c);
    propose(&mut a, &c, false);
    advance(&mut a, CHALLENGE);
    exec(&mut a, &c, ALICE, &ExecuteMsg::Settle { market_id: 1 }, 0).unwrap();

    advance(&mut a, GRACE + 1);
    assert!(expire(&mut a, &c, CAROL).is_err());
    assert_eq!(market(&a, &c).status, Status::Settled);
}

// ── конфиг и миграция ───────────────────────────────────────────────────────

#[test]
fn dispute_settings_cannot_be_zeroed() {
    let mut a = app();
    let c = setup(&mut a);
    for (bond, secs, grace) in [
        (Some(Uint128::zero()), None, None),
        (None, Some(0u64), None),
        (None, None, Some(3_600u64)),
    ] {
        let e = exec(
            &mut a,
            &c,
            ADMIN,
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
                paused: None,
                arbiter: None,
                challenge_bond: bond,
                arbiter_secs: secs,
                resolve_grace_secs: grace,
            },
            0,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("Dispute settings"), "{e}");
    }
}

#[test]
fn migrate_sets_the_dispute_settings() {
    let mut a = app();
    let c = setup(&mut a);
    let new_code = code(&mut a);
    a.migrate_contract(
        Addr::unchecked(ADMIN),
        c.clone(),
        &MigrateMsg {
            arbiter: Some("multisig".into()),
            challenge_bond: Some(Uint128::new(200_000_000_000)),
            arbiter_secs: Some(259_200),
            resolve_grace_secs: Some(604_800),
        },
        new_code,
    )
    .unwrap();

    let cfg: Config = a.wrap().query_wasm_smart(c, &QueryMsg::Config {}).unwrap();
    assert_eq!(cfg.arbiter, Some(Addr::unchecked("multisig")));
    assert_eq!(cfg.challenge_bond, Uint128::new(200_000_000_000));
    assert_eq!(cfg.arbiter_secs, 259_200);
}

/// Главная проверка миграции. Конфиг и рынки, записанные кодом 0.1.0, не
/// содержат новых полей. Без serde(default) контракт не прочитал бы ни
/// собственный конфиг, ни один существующий рынок.
#[test]
fn storage_written_by_the_old_code_still_reads() {
    let old_config = r#"{"admin":"a","resolver":"r","draw_pool":"d","treasury":"t",
        "denom":"uluna","protocol_bps":500,"creator_bps":300,"boost_bps":200,
        "creation_bond":"50","promo_fee":"200","min_bet":"10","max_bet":"1000",
        "boost_amount":"100","boost_per_week":2,"challenge_secs":3600,
        "bet_cutoff_secs":86400,"paused":false}"#;
    let cfg: Config = from_json(old_config.as_bytes()).unwrap();
    assert_eq!(cfg.arbiter, None);
    assert_eq!(cfg.challenge_bond, Uint128::zero());

    let old_market = r#"{"id":1,"creator":"c","question":"q","category":"chain",
        "spec":{"metric":"total_supply","param":null,"comparator":"lt",
          "threshold":"6000000000000","height":30240807,"criterion":"x"},
        "fees":{"protocol_bps":500,"creator_bps":300,"boost_bps":200},
        "bets_close_at":1,"resolve_after":2,"status":"settled","outcome":false,
        "reading":"r","proposed_at":3,"pot_yes":"3000000","pot_no":"2000000",
        "boost":"0","bettors_yes":1,"bettors_no":1,"bond":"1000000",
        "bond_returned":true,"promoted":false}"#;
    let m: Market = from_json(old_market.as_bytes()).unwrap();
    assert_eq!(m.status, Status::Settled);
    assert_eq!(m.spec.unit, None);
    assert_eq!(m.void_reason, None);
    assert_eq!(m.challenge_bond, Uint128::zero());
}

// ── имя сообщения ───────────────────────────────────────────────────────────

/// С 0.2.1 сообщение называется `predict`, но старое `bet` принимается как
/// синоним: кошельки и скрипты под прежнюю версию не должны сломаться.
#[test]
fn predict_is_the_name_and_bet_still_works() {
    let mut a = app();
    let c = setup(&mut a);
    let close = now(&a) + 1_000;
    exec(&mut a, &c, CREATOR, &ExecuteMsg::Create {
        question: "q".into(), category: "economy".into(),
        spec: Spec { metric: Some("total_supply".into()), param: None, comparator: Some("lt".into()),
            threshold: Some("1".into()), height: Some(1), criterion: "c".into(), unit: None },
        bets_close_at: close, resolve_after: close + CUTOFF + 1, promoted: false,
    }, BOND).unwrap();

    // Новое имя.
    let new_json = serde_json_like("predict", true);
    a.execute(Addr::unchecked(ALICE), cosmwasm_std::WasmMsg::Execute {
        contract_addr: c.to_string(), msg: new_json, funds: coins(100, DENOM),
    }.into()).unwrap();
    // Старое имя, тот же рынок.
    let old_json = serde_json_like("bet", false);
    a.execute(Addr::unchecked(BOB), cosmwasm_std::WasmMsg::Execute {
        contract_addr: c.to_string(), msg: old_json, funds: coins(200, DENOM),
    }.into()).unwrap();

    let m = market(&a, &c);
    assert_eq!(m.pot_yes, Uint128::new(100));
    assert_eq!(m.pot_no, Uint128::new(200));
}

/// Сообщение в том виде, в каком его шлёт кошелёк: {"<имя>": {...}}.
fn serde_json_like(name: &str, side: bool) -> cosmwasm_std::Binary {
    cosmwasm_std::Binary::from(format!(r#"{{"{name}":{{"market_id":1,"side":{side}}}}}"#).into_bytes())
}
