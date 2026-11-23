#![forbid(unsafe_code)]
//! Brain side of the 100 % market lots (plan rendements R5,
//! `lifescape_planner::trade_craft`): the money guide's market rows a bot
//! may work become offers of its money node, read from the precomputed
//! guides (`guides_store::current()`, never a build on demand) with the
//! cycle's asks, the units the bot holds and its free wallet; the lot a
//! resolve locks is recorded in the life's memory, what the craft run
//! made folds into it, and its product sells with motive `trade`.
//! Ironman: nothing (no offer, no lot).

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_feasible::BotState;
use lifescape_planner::guide_model::{Family, MoneyRow, Quote, WINDOW_TICKS};
use lifescape_planner::trade_craft;
use lifescape_planner::{Life, Node, NodeKind};

use crate::guides_store::GuideSet;
use crate::jobs::Job;

/// Market rows of the set a bot reads: for each F2P skill, the money view
/// of the bot's level in it, the market rows of the activities that skill
/// bands (the yields of the bot's own band), best profit first.
pub fn market_rows<'a>(set: &'a GuideSet, level_of: &dyn Fn(&str) -> u8) -> Vec<&'a MoneyRow> {
    let mut levels: Vec<u8> = lifescape_planner::guide_methods::SKILLS.iter().map(|skill| level_of(skill).clamp(1, 99)).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut out: Vec<&MoneyRow> = Vec::new();
    for level in levels {
        for row in set.money_view(Some(level)).rows.iter().filter(|row| row.family == Family::Market) {
            let banded = lifescape_planner::activity::band_skill(&row.method, &row.style).is_some_and(|skill| level_of(&skill).clamp(1, 99) == level);
            if banded {
                out.push(row);
            }
        }
    }
    out.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000));
    out
}

/// The offers of a bot (pure over the set): the market rows within its
/// reach (D22, `trade_craft::eligible` = `guide_feasible::feasible_for`
/// on `bot`), plus the bare offer of the lot its node bought for
/// (`blocked_by`) when that row left the guide (bought stock never
/// strands). Plan équilibre step 2: the cycle's quotes carry the units
/// the OTHER producers claim now (`claimed`, the live public board): a
/// product they already serve leaves no demand, no offer; a lot is
/// sized and predicted on what they leave (`trade_craft::offer_from_row`).
pub fn offers_for(set: &GuideSet, node: &Node, bot: &BotState, claimed: &dyn Fn(&str) -> f64) -> Vec<trade_craft::TradeOffer> {
    let rows = market_rows(set, bot.level_of);
    let quote = |item: &str| set.quotes.get(item).cloned().map(|quote| Quote { claimed: claimed(item).max(0.0), ..quote });
    let shop = |item: &str| crate::guides_model::npc_volume_value(item);
    let mut offers = trade_craft::offers(&rows, &quote, &shop, bot);
    if let Some(bought) = node.blocked_by.as_deref().filter(|method| trade_craft::is_trade(method)) {
        if !offers.iter().any(|offer| offer.method == bought) {
            let held = |item: &str| (bot.held)(item).max(0.0).floor() as i64;
            offers.extend(trade_craft::recipe_offer(bought, &held));
        }
    }
    offers
}

/// Units resting on the asks of `item` in the set's cycle (the market of
/// the moment the brain knows, at most `REFRESH_PERIOD` old).
pub fn cycle_asks(set: &GuideSet, item: &str) -> Option<f64> {
    set.quotes.get(item).map(|quote| quote.asks.iter().fold(0.0, |total, (_, units)| total + units.max(0.0)))
}

/// Publishes the bot's trade offers in `ctx` (a money or coin node, never
/// an Ironman): no guide set yet, no offer (logged once per pass). The
/// bot judged (D22): live levels and combat, its quests, its holdings,
/// its free wallet, the cycle's asks.
pub(crate) fn publish(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, job: &Job) {
    if !matches!(node.kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) || life.profile.ironman {
        return;
    }
    let Some(set) = crate::guides_store::current() else {
        crate::logln!("brain {}: no trade offer for {}: guides not built yet", job.account, node.id);
        return;
    };
    let levels = crate::brain_market::live_levels(live);
    let level_of = |skill: &str| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level).unwrap_or(1);
    let held = |item: &str| crate::brain_plan::holdings_count(job, item) as f64;
    let asks = |item: &str| cycle_asks(&set, item);
    let wallet = crate::brain_gear::free_coins(job, None);
    let bot = BotState { level_of: &level_of, combat: ctx.combat, quests_done: &ctx.quests_done, held: &held, coins: Some(wallet as f64), ironman: false, asks: &asks };
    let claims = crate::market_claims::claimed_by_others(&job.account);
    let claimed = |item: &str| claims.get(item).copied().unwrap_or(0) as f64;
    ctx.trades = offers_for(&set, node, &bot, &claimed);
    ctx.trade_wallet = wallet;
}

/// Ticks a lot is on its way: its craft at the guide's yield, plus one
/// window for its purchases (the claim's horizon, `market_claims`).
pub fn lot_ticks(lot: &trade_craft::TradeLot) -> f64 {
    let craft = if lot.guide_per_6000 > 0.0 { lot.lot as f64 * WINDOW_TICKS / lot.guide_per_6000 } else { WINDOW_TICKS };
    craft + WINDOW_TICKS
}

/// Products a craft report made of the trade method's product (flows of
/// cause `product`, display or debug names), 0 for any other method.
pub fn made_of(report: &Value, method: &str) -> i64 {
    let Some((product, _)) = trade_craft::recipe_of(method).and_then(lifescape_planner::method_way::method_way) else { return 0 };
    report
        .get("flows")
        .as_arr()
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("cause").as_str() == Some("product"))
                .filter(|row| row.get("name").as_str().is_some_and(|name| name == product || crate::rates_names::debug_of(name) == product))
                .filter_map(|row| row.get("qty").as_i64())
                .filter(|qty| *qty > 0)
                .sum()
        })
        .unwrap_or(0)
}

/// Motive of a listing (plan marché M1): a trade lot's product `trade`,
/// found stock `loot`, the rest `surplus`.
pub fn sale_motive(item: &str, found: bool, trade_products: &[String]) -> &'static str {
    if trade_products.iter().any(|product| product == item) {
        crate::market_lineage::TRADE
    } else if found {
        crate::market_lineage::LOOT
    } else {
        crate::market_lineage::SURPLUS
    }
}

#[cfg(test)]
mod tests {
    use super::{made_of, offers_for, sale_motive};
    use lifescape_planner::guide_model::{Confidence, Family, FlowLine, MoneyRow, Outlet, PriceBasis, Prereqs, Quote};
    use lifescape_planner::guide_read::{MoneyView, TrainingView};
    use lifescape_planner::{Node, NodeKind};

    fn line(item: &str, cause: &str, per_6000: f64, price: f64) -> FlowLine {
        FlowLine { item: item.to_string(), cause: cause.to_string(), per_6000, unit_price: price, value_6000: per_6000 * price, outlet: Outlet::Market, traded: true, basis: PriceBasis::Traded }
    }

    fn bronze_row(family: Family) -> MoneyRow {
        MoneyRow {
            family,
            method: "smelt:bronze_bar".to_string(),
            style: String::new(),
            site: None,
            tool: String::new(),
            band: 26,
            prereqs: Prereqs { levels: vec![("smithing".to_string(), 1)], ..Prereqs::default() },
            flows: vec![line("bronze_bar", "product", 100.0, 40.0), line("copper_ore", "input", -100.0, 10.0), line("tin_ore", "input", -100.0, 12.0)],
            other_drops: Default::default(),
            revenue_6000: 4000.0,
            cost_6000: 2200.0,
            profit_6000: 1800.0,
            xp_6000: Vec::new(),
            extra_ticks_per_6000: 0.0,
            stages: Vec::new(),
            routes: Vec::new(),
            confidence: Confidence::default(),
            price_confidence: "traded",
            priced: true,
        }
    }

    /// A precomputed set holding the bronze rows in the view of band 26-30.
    fn set() -> crate::guides_store::GuideSet {
        let view = MoneyView { level: Some(30), rows: vec![bronze_row(Family::Recipe), bronze_row(Family::Market)], gaps: Vec::new(), recipes: Vec::new() };
        let mut quotes = std::collections::HashMap::new();
        quotes.insert("copper_ore".to_string(), Quote { price: 10.0, traded: true, asks: vec![(10.0, 60.0)], ..Quote::default() });
        quotes.insert("tin_ore".to_string(), Quote { price: 12.0, traded: true, asks: vec![(12.0, 200.0)], ..Quote::default() });
        // Bronze bars: the last fills moved 150 at 40.
        quotes.insert("bronze_bar".to_string(), Quote { price: 40.0, traded: true, median: 40.0, fills: 5, fill_units: 150.0, ..Quote::default() });
        crate::guides_store::GuideSet {
            built_unix_ms: 0,
            built: std::time::Instant::now(),
            cycle_ms: 0,
            cycle_cpu_ms: None,
            market_wait_ticks: None,
            money_catalog: MoneyView { level: None, rows: Vec::new(), gaps: Vec::new(), recipes: Vec::new() },
            training_catalog: TrainingView { level: None, rows: Vec::new(), gaps: Vec::new() },
            money_bands: vec![(30, view)],
            training_bands: Vec::new(),
            quotes,
            stats: lifescape_json::Value::Null,
        }
    }

    /// R5: the brain's offers come from the precomputed guide, the view
    /// of the bot's own level in the recipe's skill, market rows only,
    /// with the cycle's asks and the units held.
    /// The offers of a bot at `level_of` holding `held` (D22: judged by
    /// `feasible_for` on the cycle's asks, a wallet of 10 000).
    fn offers_of(guides: &crate::guides_store::GuideSet, node: &Node, level_of: &dyn Fn(&str) -> u8, held: &dyn Fn(&str) -> f64) -> Vec<lifescape_planner::trade_craft::TradeOffer> {
        offers_claimed(guides, node, level_of, held, &|_| 0.0)
    }

    /// `offers_of` with `claimed` units of each object claimed by the
    /// other producers on the public board.
    fn offers_claimed(guides: &crate::guides_store::GuideSet, node: &Node, level_of: &dyn Fn(&str) -> u8, held: &dyn Fn(&str) -> f64, claimed: &dyn Fn(&str) -> f64) -> Vec<lifescape_planner::trade_craft::TradeOffer> {
        let asks = |item: &str| super::cycle_asks(guides, item);
        let bot = super::BotState { level_of, combat: 30, quests_done: &[], held, coins: Some(10_000.0), ironman: false, asks: &asks };
        offers_for(guides, node, &bot, claimed)
    }

    /// Plan équilibre step 2: two smelters on the same bars. The first
    /// reads the 150 bars the market took recently; once it claims 60,
    /// the second sees the 90 it leaves; once 150 are claimed, no demand
    /// is left: no 100 % market lot for the next one. The cycle's quote
    /// carries the LIVE claims of the others.
    #[test]
    fn a_trade_lot_reads_the_demand_the_other_producers_leave() {
        let guides = set();
        let node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        let smith = |skill: &str| if skill == "smithing" { 28 } else { 1 };
        let none = |_: &str| 0.0;
        let first = offers_claimed(&guides, &node, &smith, &none, &|_| 0.0);
        assert_eq!(first.len(), 1);
        assert!((first[0].demand - 150.0).abs() < 1e-9, "{}", first[0].demand);
        let second = offers_claimed(&guides, &node, &smith, &none, &|item| if item == "bronze_bar" { 60.0 } else { 0.0 });
        assert!((second[0].demand - 90.0).abs() < 1e-9, "the first smelter's 60 are on their way: {}", second[0].demand);
        let late = offers_claimed(&guides, &node, &smith, &none, &|item| if item == "bronze_bar" { 150.0 } else { 0.0 });
        assert!(late.is_empty(), "no demand left: no lot offered: {late:?}");
    }

    /// Plan équilibre step 4: ores resting at 1 coin under what a bronze
    /// bar sells for, demand left for bars: the 100 % market lot is
    /// offered, sized on the bars the market takes and bought along the
    /// asks at 1; the same ores once other producers claimed every bar:
    /// no lot.
    #[test]
    fn inputs_at_one_coin_under_their_transformation_value_make_a_trade_lot() {
        let mut guides = set();
        guides.quotes.insert("copper_ore".to_string(), Quote { price: 1.0, traded: true, asks: vec![(1.0, 5000.0)], ..Quote::default() });
        guides.quotes.insert("tin_ore".to_string(), Quote { price: 1.0, traded: true, asks: vec![(1.0, 5000.0)], ..Quote::default() });
        let node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        let smith = |skill: &str| if skill == "smithing" { 28 } else { 1 };
        let none = |_: &str| 0.0;
        let offers = offers_claimed(&guides, &node, &smith, &none, &|_| 0.0);
        assert_eq!(offers.len(), 1, "{offers:?}");
        let plan = lifescape_planner::trade_craft::lot_size(&offers[0], 10_000);
        assert!(plan.lot > 0, "{plan:?}");
        assert_eq!(plan.cost, 2 * plan.lot, "bought along the asks at 1: {plan:?}");
        let served = offers_claimed(&guides, &node, &smith, &none, &|item| if item == "bronze_bar" { 150.0 } else { 0.0 });
        assert!(served.is_empty(), "no demand left for bars: no lot: {served:?}");
    }

    #[test]
    fn the_brain_reads_its_offers_from_the_precomputed_guide() {
        let guides = set();
        let node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        let smith = |skill: &str| if skill == "smithing" { 28 } else { 1 };
        let held = |item: &str| if item == "copper_ore" { 5.0 } else { 0.0 };
        let offers = offers_of(&guides, &node, &smith, &held);
        assert_eq!(offers.len(), 1, "the market row only: {offers:?}");
        assert_eq!(offers[0].method, "trade:craft/smelt:bronze_bar");
        assert_eq!(offers[0].inputs[0].asks, vec![(10.0, 60.0)], "the cycle's asks");
        assert_eq!(offers[0].inputs[0].held, 5);
        let novice = |_: &str| 3u8;
        assert!(offers_of(&guides, &node, &novice, &held).is_empty(), "another band's view: no row read for it");
        // Bought for a lot whose row left the guide: its bare offer stays.
        let mut bought = node.clone();
        bought.blocked_by = Some("trade:craft/smelt:bronze_bar".to_string());
        let kept = offers_of(&guides, &bought, &novice, &|_| 22.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].makeable(), 22);
    }

    /// D22: the market availability a row names is judged on the cycle's
    /// asks and the bot's own stock: no tin offered and none held, no
    /// offer; a bot holding a lot of tin gets it.
    #[test]
    fn the_brain_judges_the_market_need_on_its_stock_and_the_cycle() {
        let mut guides = set();
        let need = |item: &str| lifescape_planner::guide_model::MarketNeed { item: item.to_string(), cause: "input".to_string(), per_6000: 100.0, per_lot: 14.0, asks: 0.0, recent: 0.0, shops: 0.0, priced: true };
        for (_, view) in &mut guides.money_bands {
            for row in &mut view.rows {
                row.prereqs.market = vec![need("copper_ore"), need("tin_ore")];
            }
        }
        guides.quotes.insert("tin_ore".to_string(), Quote { price: 12.0, traded: true, ..Quote::default() });
        let node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
        let smith = |skill: &str| if skill == "smithing" { 28 } else { 1 };
        assert!(offers_of(&guides, &node, &smith, &|_| 0.0).is_empty(), "no tin offered in the cycle, none held");
        let tin = |item: &str| if item == "tin_ore" { 14.0 } else { 0.0 };
        assert_eq!(offers_of(&guides, &node, &smith, &tin).len(), 1, "its own tin");
    }

    /// Bought inputs wait for the craft (never listed nor dropped); the
    /// product sells.
    #[test]
    fn bought_inputs_are_kept_and_the_product_sells() {
        let mut life = crate::brain_sales::tests::life_training(&[]);
        let mut node = Node::new("n9", NodeKind::MakeMoney, Vec::new(), 0);
        node.blocked_by = Some("trade:craft/smelt:bronze_bar".to_string());
        node.deps.push("n10".to_string());
        life.nodes.push(node);
        let keep = crate::brain_sales::working_keep(&life, &[]);
        assert!(keep.contains(&"copper_ore".to_string()) && keep.contains(&"tin_ore".to_string()), "{keep:?}");
        assert!(!keep.contains(&"bronze_bar".to_string()), "the product sells");
        let janitor = crate::brain::cleanup_keep(&life);
        assert!(janitor.contains(&"tin_ore".to_string()), "the janitor keeps them too");
    }

    #[test]
    fn a_trade_product_sells_with_its_own_motive() {
        let products = vec!["bronze_bar".to_string()];
        assert_eq!(sale_motive("bronze_bar", false, &products), "trade");
        assert_eq!(sale_motive("bronze_bar", true, &products), "trade", "a lot's product is never loot");
        assert_eq!(sale_motive("logs", true, &products), "loot");
        assert_eq!(sale_motive("logs", false, &products), "surplus");
    }

    #[test]
    fn the_craft_report_tells_what_the_lot_made() {
        let report = lifescape_json::parse(
            r#"{"flows":[{"name":"Bronze bar","cause":"product","qty":21},{"name":"Copper ore","cause":"input","qty":-22},{"name":"bronze_bar","cause":"traded","qty":5}]}"#,
        )
        .expect("report");
        assert_eq!(made_of(&report, "trade:craft/smelt:bronze_bar"), 21, "products only, by display or debug name");
        assert_eq!(made_of(&report, "smelt:bronze_bar"), 0, "only a trade lot");
    }
}
