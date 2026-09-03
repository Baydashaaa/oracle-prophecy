# oracle-prophecy

Prediction markets for [Terra Oracle Classic](https://terraoracle.io), on Terra Classic (columbus-5).

Stake LUNC on a statement about the future. Winners keep their stake and split
the losing pot. Markets whose outcome can be read from the chain settle against
the chain itself, at a block height fixed before the first bet was placed.

## Why the chain decides

A prediction market lives or dies on one question: who says what happened.

Here, for a whole class of markets, nobody does. The statement names a metric,
a comparison, a threshold and a **block height**. At settlement the value is
read from the chain at that height and compared. The answer is not announced,
it is computed, and anyone can recompute it later:

```bash
curl -s -H "x-cosmos-block-height: 30240807" \
  "https://terra-classic-lcd.publicnode.com/cosmos/bank/v1beta1/supply/by_denom?denom=uluna"
```

Block height, not a date. The oracle module revotes exchange rates every thirty
seconds, so "on Monday" is a day, not a moment - and a market you can argue
about is not a market.

Sources are always chain state: oracle module exchange rates (twenty
currencies, a weighted median of validator votes), total supply, staking ratio,
community pool, validator voting power, governance proposal outcomes. Never an
exchange price - pick an exchange and the result depends on whose exchange you
picked.

## What the contract guarantees

- **The question cannot change after bets are placed.** Metric, comparator,
  threshold and height are stored at creation and the resolver must settle
  against them.
- **Fees come out of the losing pot only.** A correct call always gets its
  full stake back.
- **Payouts wait out a challenge window.** The outcome is proposed, then a
  fixed period passes before money can move.
- **A failed reading refunds everyone.** If the metric cannot be read, or
  everybody backed the same side, the market voids and stakes come back whole.
- **Self-dealing loses money.** The creator fee is required to stay below the
  protocol fee, so betting both sides of your own market is a guaranteed loss.
- **Rounding always favours the contract.** Every division floors, so the
  contract can never promise more than it holds.

## The money

Fees are charged on the **losing pot only**, never on the winning stake.

| Share | Goes to |
|---|---|
| 5% | Protocol - half to the weekly Oracle Draw pool, half to the Treasury |
| 3% | Whoever created the market |
| 2% | Boost fund, which tops up future markets |
| 90% | Split among the winners, in proportion to stake |

Worked example. A market takes 1,000,000 LUNC: 400,000 on *yes*, 600,000 on
*no*. The outcome is *yes*.

```
losing pot          600,000
  protocol   5%      30,000   → 15,000 draw pool + 15,000 treasury
  creator    3%      18,000
  boost      2%      12,000
  winners   90%     540,000
a 100,000 stake on yes → 100,000 + 540,000 × 100,000/400,000 = 235,000
```

Creating a market costs a **bond**, returned when the market settles. It is
only lost if the market has to be voided because the criterion turned out to be
unverifiable - write a clear question and creation is free. Promotion is a
separate, non-refundable fee.

The **boost fund** pays a fixed top-up into new markets, capped per week. It is
added to the pot and goes to the winners whichever side wins: the protocol
sponsors activity, it never takes a position.

Note on transfers: Terra Classic applies a burn tax to bank sends, so a payout
arrives slightly smaller than the figure the contract computed.

## Spec format

```json
{
  "metric": "total_supply",
  "param": null,
  "comparator": "lt",
  "threshold": "6000000000000",
  "height": 30240807,
  "criterion": "bank supply of uluna at the given height"
}
```

`metric` is one of `oracle_rate`, `total_supply`, `staking_ratio`,
`community_pool`, `validator_power`, `proposal_passed`. `param` carries the
currency, validator address or proposal id where the metric needs one.
`threshold` is a string so that rates like `0.000050160711033701` keep every
digit.

Leave `metric` empty for a market the chain cannot check. Then `criterion` is
the whole agreement, and it must be specific enough that two strangers reading
it reach the same answer.

## Messages

| Execute | Who | Notes |
|---|---|---|
| `create` | anyone | bond attached; bets must close well before resolution |
| `bet` | anyone | funds attached; repeat bets add to the existing one |
| `propose` | resolver | after `resolve_after`; carries the reading |
| `challenge` | admin | inside the window; sends the market back for a second reading |
| `settle` | **anyone** | after the window; pays fees, returns the bond, opens payouts |
| `void` | admin or resolver | `bad_spec` decides whether the bond is burned |
| `claim` | anyone | winnings, or the refund on a voided market |
| `fund_boost` | anyone | tops up the boost fund |
| `update_config` | admin | cannot break the creator-below-protocol rule |

| Query | Returns |
|---|---|
| `config` | protocol settings |
| `market` | one market with pots, status and reading |
| `markets` | paged list, optionally filtered by status |
| `position` | a wallet's stakes and what it would collect |
| `boost` | fund balance and the weekly allowance |

## Build

Reproducible through the CosmWasm optimizer. `Cargo.lock` pins `base64ct`
and `zeroize` to versions the image's toolchain can parse.

```bash
docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source="$(basename "$(pwd)")_cache",target=/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0
```

Compare `artifacts/checksums.txt` with what the chain reports for the code id.

```bash
cargo test   # 24 tests: economics to the last unit, solvency, edge cases
```

## Deployments

| What | Value |
|---|---|
| code_id | 11643 |
| checksum | `2060d6c7be53693192fa517b33420677d14d494a14077b024f2b84b33aa553ab` |
| test instance | `terra1w3f09yqcna09hgc562azuze8x4qdvnzanz429cwycm84m8lygffskwcu58` |

## License

Apache-2.0
