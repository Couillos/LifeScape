//! NPCs that sell through their dialogue, no shop interface (rev 274
//! scripts, read 2026-11-13): who sells what, the coins one purchase
//! takes from the PACK, and the answers that buy (one unit per
//! conversation). The planner's `dialogue:buy` leg and the
//! `dialogue_buy` executor read this table; the merchant rule
//! (`obtain::merchant_buy_allowed`) decides which objects a bot may buy
//! (`allowed`).
//!
//! Answers are the option texts of the buying branch, in the order the
//! pages ask them (`multi`/`p_choice` lines of each script):
//! - Karim `kebab_seller.rs2`: "Yes please." (1 coin);
//! - the silk trader `silk_trader.rs2`: "How much are they?" then "Okay,
//!   that sounds good." (3 coins);
//! - Baraek `baraek.rs2`: "Can you sell me some furs?" then "Yeah, Okay,
//!   here you go." (20 coins);
//! - the Varrock and Port Sarim bartenders (`bartender.rs2`) a beer for 2;
//! - the Rising Sun barmaid (`area_falador/barmaid.rs2`): "Hi, what ales
//!   are you serving?" then the ale (2 or 3 coins);
//! - Ned `ned.rs2`: "Yes, I would like some rope." then "Okay, please sell
//!   me some rope." (15 coins).

/// One NPC's dialogue sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueShop {
    /// NPC debugname (`kebab_seller`).
    pub npc: &'static str,
    /// Object debugname (`kebab`).
    pub item: &'static str,
    /// Coins one purchase takes from the pack.
    pub cost: i64,
    /// Options to pick, page by page.
    pub answers: &'static [&'static str],
    /// The content script, for the record.
    pub file: &'static str,
}

const SHOPS: &[DialogueShop] = &[
    DialogueShop { npc: "kebab_seller", item: "kebab", cost: 1, answers: &["Yes please."], file: "areas/area_alkharid/scripts/kebab_seller.rs2" },
    DialogueShop { npc: "silk_trader", item: "silk", cost: 3, answers: &["How much are they?", "Okay, that sounds good."], file: "areas/area_alkharid/scripts/silk_trader.rs2" },
    DialogueShop { npc: "baraek", item: "fur", cost: 20, answers: &["Can you sell me some furs?", "Yeah, Okay, here you go."], file: "areas/area_varrock/scripts/baraek.rs2" },
    DialogueShop { npc: "jollyboar_bartender", item: "beer", cost: 2, answers: &["I'll have a beer please."], file: "areas/area_varrock/scripts/bartender.rs2" },
    DialogueShop { npc: "bluemoon_bartender", item: "beer", cost: 2, answers: &["A glass of your finest ale please."], file: "areas/area_varrock/scripts/bartender.rs2" },
    DialogueShop { npc: "donkey_bartender", item: "beer", cost: 2, answers: &["Yes please!"], file: "areas/area_varrock/scripts/bartender.rs2" },
    DialogueShop { npc: "rustyanchor_bartender", item: "beer", cost: 2, answers: &["Could I buy a beer please?"], file: "areas/area_port_sarim/scripts/bartender.rs2" },
    DialogueShop {
        npc: "risingsun_barmaid",
        item: "asgarnian_ale",
        cost: 2,
        answers: &["Hi, what ales are you serving?", "One Asgarnian Ale, please."],
        file: "areas/area_falador/scripts/barmaid.rs2",
    },
    DialogueShop {
        npc: "risingsun_barmaid",
        item: "wizards_mind_bomb",
        cost: 2,
        answers: &["Hi, what ales are you serving?", "I'll try the Mind Bomb."],
        file: "areas/area_falador/scripts/barmaid.rs2",
    },
    DialogueShop {
        npc: "risingsun_barmaid",
        item: "dwarven_stout",
        cost: 3,
        answers: &["Hi, what ales are you serving?", "Can I have a Dwarven Stout?"],
        file: "areas/area_falador/scripts/barmaid.rs2",
    },
    DialogueShop {
        npc: "ned",
        item: "rope",
        cost: 15,
        answers: &["Yes, I would like some rope.", "Okay, please sell me some rope."],
        file: "areas/area_draynor/scripts/ned.rs2",
    },
];

/// Every dialogue sale in the table (merchant rule not applied).
pub fn all() -> &'static [DialogueShop] {
    SHOPS
}

/// F2P spawns (x, z, level) of the selling NPC.
pub fn sites(shop: &DialogueShop) -> Vec<(i64, i64, i64)> {
    crate::npcs_by_name(shop.npc)
        .iter()
        .filter(|npc| npc.members != Some(true))
        .flat_map(|npc| npc.spawns.iter().filter(|spot| !spot.members && crate::free_zones::is_free(spot.x, spot.z)).map(|spot| (spot.x, spot.z, spot.level)))
        .collect()
}

/// Dialogue sales of `item` a bot may use (merchant rule, user decision
/// 2026-11-13: only what F2P gives no other way) with an F2P seller.
pub fn allowed(item: &str) -> Vec<&'static DialogueShop> {
    if !crate::obtain::merchant_buy_allowed(item) {
        return Vec::new();
    }
    SHOPS.iter().filter(|shop| shop.item == item && !sites(shop).is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::{all, allowed, sites};

    /// Every row names a real F2P NPC and object, and its answers.
    #[test]
    fn rows_name_real_npcs_and_objects() {
        for shop in all() {
            assert!(!sites(shop).is_empty(), "{} has no F2P spawn", shop.npc);
            assert!(!crate::objs_by_name(shop.item).is_empty(), "{} unknown", shop.item);
            assert!(!shop.answers.is_empty() && shop.cost > 0, "{shop:?}");
            assert!(
                crate::script_sources::dialogue_buys().iter().any(|buy| buy.npc == shop.npc && buy.obj == shop.item && buy.cost == shop.cost),
                "{} {} {} not in the dialogue dump",
                shop.npc,
                shop.item,
                shop.cost
            );
        }
    }

    /// Karim's kebab is sold only through his dialogue in F2P (no recipe,
    /// no drop): the merchant rule lets a bot buy it; a producible object
    /// never comes from a dialogue.
    #[test]
    fn the_kebab_is_a_dialogue_buy() {
        let kebab = allowed("kebab");
        assert_eq!(kebab.len(), 1, "{kebab:?}");
        assert_eq!((kebab[0].npc, kebab[0].cost, kebab[0].answers), ("kebab_seller", 1, &["Yes please."][..]));
        assert!(allowed("bronze_bar").is_empty());
        let open: Vec<&str> = all().iter().filter(|shop| !allowed(shop.item).is_empty()).map(|shop| shop.item).collect();
        println!("dialogue buys allowed: {open:?}");
    }
}
