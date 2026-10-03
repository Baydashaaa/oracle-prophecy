// Аудит MKT-06 (0.2.5): индекс по статусу, ограниченный перебор, порядок от
// новых к старым и заполнение индекса миграцией.

use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
use cosmwasm_std::{coins, from_json, Env, Uint128};

use oracle_prophecy::contract::{execute, instantiate, migrate, query};
use oracle_prophecy::msg::{ExecuteMsg, InstantiateMsg, MarketsResponse, MigrateMsg, QueryMsg};
use oracle_prophecy::state::{status_key, Spec, Status, BY_STATUS};

const DENOM: &str = "uluna";
const BOND: u128 = 50;

fn init_msg() -> InstantiateMsg {
    InstantiateMsg {
        admin: Some("admin".into()),
        resolver: "resolver".into(),
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
        challenge_secs: 3_600,
        bet_cutoff_secs: 86_400,
        arbiter: Some("arbiter".into()),
        challenge_bond: Uint128::new(50),
        arbiter_secs: 7_200,
        resolve_grace_secs: 604_800,
    }
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

fn create(deps: cosmwasm_std::DepsMut, env: &Env) {
    let close = env.block.time.seconds() + 1_000;
    execute(
        deps,
        env.clone(),
        mock_info("creator", &coins(BOND, DENOM)),
        ExecuteMsg::Create {
            question: "supply below 6T".into(),
            category: "economy".into(),
            spec: spec(),
            bets_close_at: close,
            resolve_after: close + 86_401,
            promoted: false,
        },
    )
    .unwrap();
}

fn void(deps: cosmwasm_std::DepsMut, env: &Env, id: u64) {
    execute(
        deps,
        env.clone(),
        mock_info("admin", &[]),
        ExecuteMsg::Void { market_id: id, bad_spec: false, reason: "test".into() },
    )
    .unwrap();
}

fn page(
    deps: cosmwasm_std::Deps,
    status: Option<Status>,
    start_after: Option<u64>,
    limit: u32,
    descending: bool,
) -> Vec<u64> {
    let r: MarketsResponse = from_json(
        query(
            deps,
            mock_env(),
            QueryMsg::Markets { status, start_after, limit: Some(limit), descending: Some(descending) },
        )
        .unwrap(),
    )
    .unwrap();
    r.markets.into_iter().map(|m| m.id).collect()
}

/// 120 рынков, каждый второй из первых ста аннулирован. Открытые находятся
/// по индексу, в том числе самые новые за длинной закрытой историей.
#[test]
fn status_pages_follow_the_index() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    instantiate(deps.as_mut(), env.clone(), mock_info("admin", &[]), init_msg()).unwrap();
    for _ in 0..120 {
        create(deps.as_mut(), &env);
    }
    for id in (2..=100).step_by(2) {
        void(deps.as_mut(), &env, id);
    }

    // Открытые: 1,3,...,99 и 101..=120 = 50 + 20.
    let mut open = vec![];
    let mut after = None;
    loop {
        let p = page(deps.as_ref(), Some(Status::Open), after, 50, false);
        if p.is_empty() {
            break;
        }
        after = p.last().copied();
        open.extend(p);
    }
    assert_eq!(open.len(), 70);
    assert!(open.contains(&120) && open.contains(&101) && !open.contains(&2));

    // Аннулированные из индекса, а не из открытых.
    let v = page(deps.as_ref(), Some(Status::Void), None, 50, false);
    assert_eq!(v.len(), 50);
    assert!(v.iter().all(|id| id % 2 == 0));
    // Старые записи индекса убраны при смене статуса.
    assert!(!BY_STATUS.has(&deps.storage, (status_key(&Status::Open), 2)));
}

#[test]
fn descending_pages_start_from_the_newest() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    instantiate(deps.as_mut(), env.clone(), mock_info("admin", &[]), init_msg()).unwrap();
    for _ in 0..30 {
        create(deps.as_mut(), &env);
    }
    assert_eq!(page(deps.as_ref(), None, None, 3, true), vec![30, 29, 28]);
    assert_eq!(page(deps.as_ref(), None, Some(28), 3, true), vec![27, 26, 25]);
    assert_eq!(page(deps.as_ref(), Some(Status::Open), Some(3), 5, true), vec![2, 1]);
    assert_eq!(page(deps.as_ref(), Some(Status::Open), Some(3), 5, false)[0], 4);
}

/// Рынки, записанные до 0.2.5, в индексе не числятся. Миграция их туда
/// вносит, и запрос по статусу их видит.
#[test]
fn migration_fills_the_index_for_old_markets() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    instantiate(deps.as_mut(), env.clone(), mock_info("admin", &[]), init_msg()).unwrap();
    for _ in 0..5 {
        create(deps.as_mut(), &env);
    }
    void(deps.as_mut(), &env, 4);
    // Так выглядит хранилище до 0.2.5: индекса нет.
    for (st, id) in [(Status::Open, 1), (Status::Open, 2), (Status::Open, 3), (Status::Void, 4), (Status::Open, 5)] {
        BY_STATUS.remove(&mut deps.storage, (status_key(&st), id));
    }
    assert!(page(deps.as_ref(), Some(Status::Open), None, 50, false).is_empty());

    let res = migrate(
        deps.as_mut(),
        env,
        MigrateMsg {
            challenge_secs: None,
            arbiter: None,
            challenge_bond: None,
            arbiter_secs: None,
            resolve_grace_secs: None,
        },
    )
    .unwrap();
    assert!(res.attributes.iter().any(|a| a.key == "markets_indexed"));
    assert_eq!(page(deps.as_ref(), Some(Status::Open), None, 50, false), vec![1, 2, 3, 5]);
    assert_eq!(page(deps.as_ref(), Some(Status::Void), None, 50, false), vec![4]);
}
