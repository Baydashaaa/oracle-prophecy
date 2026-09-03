use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Uint128};
use cw_storage_plus::{Item, Map};

/// Настройки протокола. Меняются админом, но не задним числом: у каждого
/// рынка своя копия долей на момент создания (см. `Market::fees`), иначе
/// изменение комиссии переписывало бы условия уже открытых рынков.
#[cw_serde]
pub struct Config {
    pub admin: Addr,
    /// Кто приносит исход рынка. Проверить его может любой - контракт хранит
    /// спецификацию показателя и высоту, - но записать может только он.
    pub resolver: Addr,
    /// Куда уходит доля розыгрыша: контракт недельного пула.
    pub draw_pool: Addr,
    pub treasury: Addr,
    pub denom: String,

    /// Доли берутся ТОЛЬКО из проигравшего банка. Победитель забирает свою
    /// ставку целиком - это правило, а не следствие расчёта.
    pub protocol_bps: u64, // 500  = 5%, пополам между розыгрышем и казной
    pub creator_bps: u64,  // 300  = 3%
    pub boost_bps: u64,    // 200  = 2%, копится в фонде доплат

    /// Залог за создание рынка. Возвращается, если рынок нормально
    /// рассчитан; сгорает в фонд доплат, если рынок аннулирован из-за
    /// непроверяемой формулировки.
    pub creation_bond: Uint128,
    /// Плата за продвижение. Невозвратна.
    pub promo_fee: Uint128,

    pub min_bet: Uint128,
    /// Потолок ставки. При малом числе участников один кошелёк не должен
    /// в одиночку двигать банк - иначе остальным неинтересно.
    pub max_bet: Uint128,

    /// Сколько добавляет казна к банку нового рынка и сколько таких доплат
    /// разрешено за неделю. Расход ограничен заранее.
    pub boost_amount: Uint128,
    pub boost_per_week: u64,

    /// Окно оспаривания после объявления исхода, секунды. Выплаты открыты
    /// только после него.
    pub challenge_secs: u64,
    /// Ставки закрываются раньше измерения на это время. Отсечка против
    /// того, чтобы ставить, когда исход уже почти известен.
    pub bet_cutoff_secs: u64,

    pub paused: bool,
}

/// Копия долей на момент создания рынка.
#[cw_serde]
pub struct Fees {
    pub protocol_bps: u64,
    pub creator_bps: u64,
    pub boost_bps: u64,
}

/// Как проверяется исход. Контракт эти поля не интерпретирует - он их
/// хранит, показывает и требует от резолвера сослаться на них при
/// объявлении. Вся ценность в том, что спецификация зафиксирована ДО ставок
/// и её нельзя подменить после.
#[cw_serde]
pub struct Spec {
    /// Для расчёта по цепочке: `oracle_rate`, `total_supply`, `staking_ratio`,
    /// `community_pool`, `validator_power`, `proposal_passed`.
    /// Пусто - рынок разрешается людьми по тексту `criterion`.
    pub metric: Option<String>,
    /// Параметр показателя: валюта, адрес валидатора, номер предложения.
    pub param: Option<String>,
    /// `gt`, `lt`, `gte`, `lte`. Для дискретных показателей не нужен.
    pub comparator: Option<String>,
    /// Порог в исходных единицах показателя, строкой - чтобы не терять
    /// точность на дробных курсах вида 0.000050160711033701.
    pub threshold: Option<String>,
    /// Высота блока, на которой снимается значение. Именно высота, а не
    /// дата: курс переголосовывается каждые тридцать секунд, и «первого
    /// сентября» - это сутки, а не момент.
    pub height: Option<u64>,
    /// Человеческий критерий для рынков, которые цепочка не проверяет.
    pub criterion: String,
}

#[cw_serde]
pub enum Status {
    /// Принимает ставки.
    Open,
    /// Ставки закрыты, ждём исхода.
    Locked,
    /// Исход объявлен, идёт окно оспаривания.
    Proposed,
    /// Окно прошло, выплаты открыты.
    Settled,
    /// Ставки возвращаются целиком. Причины: исход не читается, ставки были
    /// только с одной стороны, формулировка непроверяема.
    Void,
}

#[cw_serde]
pub struct Market {
    pub id: u64,
    pub creator: Addr,
    pub question: String,
    pub category: String,
    pub spec: Spec,
    pub fees: Fees,

    /// Момент закрытия приёма ставок.
    pub bets_close_at: u64,
    /// Момент, начиная с которого исход можно объявлять.
    pub resolve_after: u64,

    pub status: Status,
    /// true = сбылось. Заполняется вместе с объявлением исхода.
    pub outcome: Option<bool>,
    /// Что именно прочитал резолвер: значение и высота. Хранится ради
    /// проверяемости, контракт этим не пользуется.
    pub reading: Option<String>,
    pub proposed_at: Option<u64>,

    /// Банки сторон и доплата казны.
    pub pot_yes: Uint128,
    pub pot_no: Uint128,
    pub boost: Uint128,

    pub bettors_yes: u64,
    pub bettors_no: u64,

    /// Залог создателя: возвращается при расчёте, теряется при аннулировании
    /// по вине формулировки.
    pub bond: Uint128,
    pub bond_returned: bool,
    pub promoted: bool,
}

impl Market {
    /// Банк, который делится: проигравшая сторона плюс доплата казны.
    /// Своя ставка победителя сюда не входит - он забирает её отдельно.
    pub fn losing_pot(&self) -> Uint128 {
        match self.outcome {
            Some(true) => self.pot_no + self.boost,
            Some(false) => self.pot_yes + self.boost,
            None => Uint128::zero(),
        }
    }

    pub fn winning_pot(&self) -> Uint128 {
        match self.outcome {
            Some(true) => self.pot_yes,
            Some(false) => self.pot_no,
            None => Uint128::zero(),
        }
    }
}

/// Ставка одного кошелька на одной стороне. Повторная ставка увеличивает
/// сумму, а не создаёт вторую запись, - иначе выплату пришлось бы собирать
/// обходом, а он не помещается в газ на большом рынке.
#[cw_serde]
pub struct Bet {
    pub amount: Uint128,
    pub claimed: bool,
}

pub const CONFIG: Item<Config> = Item::new("config");
pub const NEXT_ID: Item<u64> = Item::new("next_id");
/// Накопленный фонд доплат. Тратится на новые рынки, пополняется долей
/// комиссии и сгоревшими залогами.
pub const BOOST_FUND: Item<Uint128> = Item::new("boost_fund");
/// Сколько доплат выдано на текущей неделе и когда неделя началась.
pub const BOOST_WEEK: Item<(u64, u64)> = Item::new("boost_week");

pub const MARKETS: Map<u64, Market> = Map::new("markets");
/// (id рынка, сторона, кошелёк) -> ставка. Сторона отдельным ключом:
/// один кошелёк может стоять на обеих, и это не запрещено - он просто
/// теряет комиссию с проигравшей половины.
pub const BETS: Map<(u64, u8, &Addr), Bet> = Map::new("bets");

/// Сторона ключом карты. `bool` не реализует PrimaryKey в cw-storage-plus,
/// поэтому в ключ идёт байт: 1 = да, 0 = нет.
pub fn side_key(side: bool) -> u8 {
    if side { 1 } else { 0 }
}
