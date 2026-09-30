// Тесты исправлений по аудиту 29.09.2026 (0.2.4).
//
// MKT-01: контракт отклоняет спецификацию с посторонними значениями.
// MKT-03: правила спора фиксируются в рынке; окно оспаривания не бывает
// нулевым; оспоренный рынок нельзя аннулировать напрямую, объявленный -
// только админом.

use cosmwasm_std::{coins, Addr, Uint128};
use cw_multi_test::{App, AppBuilder, ContractWrapper, Executor};

use oracle_prophecy::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use oracle_prophecy::state::{Market, Spec, Status};

const DENOM: &str = "uluna";
const ADMIN: &str = "admin";
const RESOLVER: &str = "resolver";
const ARBITER: &str = "arbiter";
const CREATOR: &str = "creator";
const ALICE: &str = "alice";
const BOB: &str = "bob";
const CAROL: &str = "carol";

const BOND: u128 = 50;
const CUTOFF: u64 = 86_400;
const CHALLENGE: u64 = 3_600;
const CH_BOND: u128 = 50;
const ARB_SECS: u64 = 7_200;

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
            draw_pool: "draw_pool".into(),
            treasury: "treasury".into(),
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
            arbiter_secs: ARB_SECS,
            resolve_grace_secs: 604_800,
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
        unit: Some("uluna".into()),
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

fn try_create(app: &mut App, c: &Addr, category: &str, spec: Spec) -> anyhow::Result<()> {
    let close = now(app) + 1_000;
    app.execute_contract(
        Addr::unchecked(CREATOR),
        c.clone(),
        &ExecuteMsg::Create {
            question: "supply below 6T".into(),
            category: category.into(),
            spec,
            bets_close_at: close,
            resolve_after: close + CUTOFF + 1,
            promoted: false,
        },
        &coins(BOND, DENOM),
    )?;
    Ok(())
}

fn market(app: &App, c: &Addr) -> Market {
    app.wrap()
        .query_wasm_smart(c.clone(), &QueryMsg::Market { market_id: 1 })
        .unwrap()
}

/// Рынок с двумя сторонами и объявленным исходом.
fn proposed(app: &mut App, c: &Addr) {
    try_create(app, c, "economy", spec()).unwrap();
    for (who, side) in [(ALICE, true), (BOB, false)] {
        app.execute_contract(
            Addr::unchecked(who),
            c.clone(),
            &ExecuteMsg::Predict { market_id: 1, side },
            &coins(100, DENOM),
        )
        .unwrap();
    }
    advance(app, 1_000 + CUTOFF + 2);
    app.execute_contract(
        Addr::unchecked(RESOLVER),
        c.clone(),
        &ExecuteMsg::Propose {
            market_id: 1,
            outcome: true,
            reading: "supply 5.4T at height 30400000".into(),
        },
        &[],
    )
    .unwrap();
}

fn challenge(app: &mut App, c: &Addr, amount: u128) -> anyhow::Result<()> {
    app.execute_contract(
        Addr::unchecked(CAROL),
        c.clone(),
        &ExecuteMsg::Challenge {
            market_id: 1,
            reading: "supply is 6.1T".into(),
        },
        &coins(amount, DENOM),
    )?;
    Ok(())
}

fn void(app: &mut App, c: &Addr, who: &str) -> anyhow::Result<()> {
    app.execute_contract(
        Addr::unchecked(who),
        c.clone(),
        &ExecuteMsg::Void {
            market_id: 1,
            bad_spec: false,
            reason: "test".into(),
        },
        &[],
    )?;
    Ok(())
}

fn update(app: &mut App, c: &Addr, challenge_secs: Option<u64>, challenge_bond: Option<u128>) -> anyhow::Result<()> {
    app.execute_contract(
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
            challenge_secs,
            bet_cutoff_secs: None,
            paused: None,
            arbiter: None,
            challenge_bond: challenge_bond.map(Uint128::new),
            arbiter_secs: None,
            resolve_grace_secs: None,
        },
        &[],
    )?;
    Ok(())
}

// ── MKT-01 ──────────────────────────────────────────────────────────────────

#[test]
fn html_in_the_spec_is_rejected() {
    let mut a = app();
    let c = setup(&mut a);
    let x = "<img src=x onerror=alert(1)>";

    let mut s = spec();
    s.comparator = Some(x.into());
    assert!(try_create(&mut a, &c, "economy", s).unwrap_err().root_cause().to_string().contains("comparator"));

    let mut s = spec();
    s.threshold = Some(x.into());
    assert!(try_create(&mut a, &c, "economy", s).unwrap_err().root_cause().to_string().contains("threshold"));

    let mut s = spec();
    s.param = Some(x.into());
    assert!(try_create(&mut a, &c, "economy", s).unwrap_err().root_cause().to_string().contains("param"));

    let mut s = spec();
    s.metric = Some(x.into());
    assert!(try_create(&mut a, &c, "economy", s).unwrap_err().root_cause().to_string().contains("metric"));

    let mut s = spec();
    s.unit = Some(x.into());
    assert!(try_create(&mut a, &c, "economy", s).unwrap_err().root_cause().to_string().contains("unit"));

    for cat in [x, "Economy", "constructor ", "", "a-very-long-category-name-here"] {
        assert!(
            try_create(&mut a, &c, cat, spec()).unwrap_err().root_cause().to_string().contains("category"),
            "category {cat:?} should be rejected"
        );
    }
}

#[test]
fn ordinary_specs_still_pass() {
    let mut a = app();
    let c = setup(&mut a);
    try_create(&mut a, &c, "economy", spec()).unwrap();

    let mut s = spec();
    s.metric = Some("oracle_rate".into());
    s.param = Some("uusd".into());
    s.comparator = Some("gte".into());
    s.threshold = Some("0.000050160711033701".into());
    s.unit = Some("rate".into());
    try_create(&mut a, &c, "crypto", s).unwrap();

    let mut s = spec();
    s.metric = Some("proposal_passed".into());
    s.param = Some("12345".into());
    s.comparator = None;
    s.threshold = None;
    s.unit = None;
    try_create(&mut a, &c, "governance", s).unwrap();

    // Рынок на реальное событие: показателя нет, ссылка на источник в param.
    let mut s = spec();
    s.metric = None;
    s.param = Some("src=fdo;event=12345;tpl=home_win".into());
    s.comparator = None;
    s.threshold = None;
    s.height = None;
    s.unit = None;
    try_create(&mut a, &c, "sport", s).unwrap();
}

#[test]
fn bad_numbers_are_rejected() {
    let mut a = app();
    let c = setup(&mut a);
    for t in ["-5", "1.2.3", ".5", "5.", "1e9", "0x10", "12 000", ""] {
        let mut s = spec();
        s.threshold = Some(t.into());
        assert!(try_create(&mut a, &c, "economy", s).is_err(), "threshold {t:?} should be rejected");
    }
}

// ── MKT-03: окно и залог ────────────────────────────────────────────────────

#[test]
fn challenge_window_cannot_be_zero_or_below_an_hour() {
    let mut a = app();
    let c = setup(&mut a);
    for secs in [0, 1, 3_599] {
        let e = update(&mut a, &c, Some(secs), None).unwrap_err();
        assert!(e.root_cause().to_string().contains("challenge_secs"));
    }
    update(&mut a, &c, Some(3_600), None).unwrap();
}

#[test]
fn a_market_keeps_its_rules_when_the_config_changes() {
    let mut a = app();
    let c = setup(&mut a);
    proposed(&mut a, &c);

    let r = market(&a, &c).rules.expect("new markets carry their rules");
    assert_eq!(r.challenge_secs, CHALLENGE);
    assert_eq!(r.challenge_bond, Uint128::new(CH_BOND));
    assert_eq!(r.arbiter, Addr::unchecked(ARBITER));

    // Админ поднимает залог в десять раз и удлиняет окно: для уже
    // объявленного рынка ничего не меняется.
    update(&mut a, &c, Some(CHALLENGE * 10), Some(CH_BOND * 10)).unwrap();

    // Залог прежний, не новый.
    assert!(challenge(&mut a, &c, CH_BOND * 10).is_err());
    challenge(&mut a, &c, CH_BOND).unwrap();
    assert_eq!(market(&a, &c).status, Status::Disputed);
}

#[test]
fn a_longer_window_in_the_config_does_not_hold_an_old_market() {
    let mut a = app();
    let c = setup(&mut a);
    proposed(&mut a, &c);
    update(&mut a, &c, Some(CHALLENGE * 10), None).unwrap();

    advance(&mut a, CHALLENGE + 1);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Settle { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(market(&a, &c).status, Status::Settled);
}

// ── MKT-03: кто может аннулировать ──────────────────────────────────────────

#[test]
fn the_resolver_cannot_void_its_own_proposal() {
    let mut a = app();
    let c = setup(&mut a);
    proposed(&mut a, &c);
    assert!(void(&mut a, &c, RESOLVER).is_err());
    void(&mut a, &c, ADMIN).unwrap();
    assert_eq!(market(&a, &c).status, Status::Void);
}

#[test]
fn nobody_voids_a_disputed_market_directly() {
    let mut a = app();
    let c = setup(&mut a);
    proposed(&mut a, &c);
    challenge(&mut a, &c, CH_BOND).unwrap();
    for who in [RESOLVER, ADMIN] {
        let e = void(&mut a, &c, who).unwrap_err();
        assert!(e.root_cause().to_string().contains("disputed"), "{who}: {e}");
    }
    // Молчащий арбитр по-прежнему ведёт к аннулированию через Expire.
    advance(&mut a, ARB_SECS + 1);
    a.execute_contract(
        Addr::unchecked(ALICE),
        c.clone(),
        &ExecuteMsg::Expire { market_id: 1 },
        &[],
    )
    .unwrap();
    assert_eq!(market(&a, &c).status, Status::Void);
}

#[test]
fn the_resolver_can_still_void_before_a_proposal() {
    let mut a = app();
    let c = setup(&mut a);
    try_create(&mut a, &c, "economy", spec()).unwrap();
    void(&mut a, &c, RESOLVER).unwrap();
    assert_eq!(market(&a, &c).status, Status::Void);
}
