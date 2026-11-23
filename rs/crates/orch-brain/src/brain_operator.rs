#![forbid(unsafe_code)]
//! Operator ambition switches (user 2026-11-18: « créé l'ambition alcher
//! et donne-la à Ryan Mason »): `orchestrator ambition <compte> <kind>`
//! files the switch here; the brain applies it at the start of that
//! account's next pass, on the life it just loaded (the brain is the only
//! writer of a life file: no race with a pass in flight). Kinds: the
//! alchemist (`alcher`), and every trade the fleet draws (user
//! 2026-11-20: « trouve les rôles idéaux pour ces 11 comptes », the
//! retired market makers placed on the piles to consume).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
use lifescape_json::Value;
use lifescape_planner::ALCHER;
#[cfg(test)]
use lifescape_planner::Life;

fn pending() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kinds an operator may hand out by this command.
pub const OPERATOR_KINDS: &[&str] = &[ALCHER];

/// `ambition <compte> natural` (user 2026-11-21): the operator account
/// joins the natural balance: pin or operator type removed, the ambition
/// given by the A4 value like any bot's, then ordinary (A2).
pub const NATURAL: &str = lifescape_planner::upkeep_role::NATURAL;

/// A trade the fleet draws (catalogue kind with a ladder, never the
/// market maker): an operator may hand it out too.
fn drawable_trade(kind: &str) -> bool {
    lifescape_planner::upkeep_role::drawable_trade(kind)
}

/// Typed refusal of a switch before it is filed: a protected account takes
/// no operator kind (overall for good), a market maker no `natural`
/// (its own retirement gives it a trade). `None`: accepted.
pub(crate) fn refusal(account: &str, kind: &str, maker: bool) -> Option<String> {
    if lifescape_planner::protected(account) {
        return Some(format!("{account} is a protected account (overall for good): `ambition {account} {kind}` refused"));
    }
    if kind == NATURAL && maker {
        return Some(format!("{account} is a market maker: `ambition {account} natural` refused (its retirement gives it a trade)"));
    }
    None
}

/// File a switch for `account` (typed refusal for an unknown kind).
pub fn request(account: &str, kind: &str) -> Result<(), String> {
    if account.is_empty() {
        return Err("ambition <compte> <kind>: no account".to_string());
    }
    let maker = crate::brain::load_life(account).is_some_and(|life| lifescape_planner::maker_role(&life).is_some());
    if let Some(error) = refusal(account, kind, maker) {
        return Err(error);
    }
    if kind != NATURAL && !OPERATOR_KINDS.contains(&kind) && !drawable_trade(kind) {
        return Err(format!("unknown operator ambition {kind:?} (known: {}, {NATURAL} or a drawn trade)", OPERATOR_KINDS.join(", ")));
    }
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.to_string(), kind.to_string());
    crate::logln!("brain {account}: operator ambition {kind} filed, applied at its next idle pass");
    Ok(())
}

/// The switch filed for this account, taken out of the table (the brain
/// applies it at the start of its next pass: `brain_upkeep_drive::apply_pending`,
/// the decision is `lifescape_planner::upkeep_role::operator_step`).
pub(crate) fn take_pending(account: &str) -> Option<String> {
    pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account)
}

/// `apply_pending` over the daemon's world without a worker (the unit tests).
#[cfg(test)]
pub(crate) fn apply_pending(life: &mut Life, live: &Value) -> bool {
    crate::brain_upkeep_drive::apply_pending(&crate::brain_upkeep_fx::LiveUpkeepFx::bare(live), life)
}

#[cfg(test)]
mod tests {
    fn live() -> lifescape_json::Value {
        use lifescape_json::{object, Value};
        let skill = |id: i64, level: i64| object(vec![("id", Value::Int(id)), ("level", Value::Int(level))]);
        object(vec![("skills", Value::Arr((0..=20).map(|id| skill(id, 30)).collect()))])
    }

    fn life_holding(account: &str, held: lifescape_planner::Ambition) -> lifescape_planner::Life {
        use lifescape_planner::{Archetype, GoalKind, Profile};
        lifescape_planner::Life {
            schema: 1,
            account: account.to_string(),
            created_at: 0,
            seed: 11,
            profile: Profile::new(false, 0.5, 0.5, 11, Archetype::Producer),
            goal: GoalKind::MakeMoney,
            nodes: Vec::new(),
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: Some(held),
            goal_origin: None,
            memory: Default::default(),
        }
    }

    /// User 2026-11-21 (« les 6 opérateurs rejoignent l'équilibre
    /// naturel »): `ambition <compte> natural` is filed for an ordinary
    /// account; a protected account refuses EVERY operator kind (typed
    /// error), a market maker refuses `natural`.
    #[test]
    fn natural_is_filed_and_protected_accounts_refuse_every_kind() {
        for name in ["Zezima", "n0valyfe", "YOGOSUN"] {
            for kind in ["natural", "alcher", "cook", "fisher"] {
                let error = super::request(name, kind).expect_err(name);
                assert!(error.contains("protected"), "{name} {kind}: {error}");
            }
        }
        assert!(super::request("t_natural_owen", "natural").is_ok());
        super::pending().lock().unwrap().remove("t_natural_owen");
        assert!(super::refusal("t_maker", "natural", true).is_some_and(|error| error.contains("market maker")));
        assert!(super::refusal("t_maker", "cook", true).is_none(), "only natural is refused to a maker here");
    }

    /// A switch already filed for a protected account (an older file) is
    /// dropped at the pass: nothing abandoned, no history, same ambition.
    #[test]
    fn a_filed_switch_never_touches_a_protected_life() {
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        for kind in ["alcher", "cook", "natural"] {
            let overall = lifescape_planner::new_ambition(lifescape_planner::OVERALL, "protected:overall", &now, 11, &[], stamp).expect("overall");
            let mut zezima = life_holding("Zezima", overall);
            let mut node = lifescape_planner::Node::new("n1", lifescape_planner::NodeKind::ReachLevel { skill: "mining".to_string(), level: 5 }, Vec::new(), 5);
            node.status = lifescape_planner::NodeStatus::Ready;
            zezima.nodes.push(node);
            let before = zezima.clone();
            super::pending().lock().unwrap().insert("Zezima".to_string(), kind.to_string());
            super::apply_pending(&mut zezima, &live());
            assert_eq!(zezima, before, "{kind}: life untouched");
            assert!(!super::pending().lock().unwrap().contains_key("Zezima"), "{kind}: the filed switch is dropped");
        }
    }

    /// A pinned account's role bans (the cause of the operator role) are
    /// lifted by `natural`, whatever the ambition it holds.
    #[test]
    fn natural_lifts_the_role_bans() {
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        let pinned = lifescape_planner::pinned_ambition("smithing", 99, &now, 11, &[], stamp).expect("pin");
        let mut owen = life_holding("t_natural_ban", pinned);
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
        let cause = format!("stuck:smithing@6000:contract error (node n1, {})", lifescape_planner::OPERATOR_ROLE_CAUSE);
        owen.history.push(lifescape_planner::HistoryEntry { node: "goal".to_string(), started: secs, ended: secs, outcome: cause });
        super::request("t_natural_ban", "natural").expect("filed");
        assert!(super::apply_pending(&mut owen, &live()));
        assert!(owen.history.iter().any(|entry| entry.outcome.starts_with("unban:smithing")), "ban lifted: {:?}", owen.history);
    }

    /// The draw fails: nothing is written (pin, rule, nodes, history).
    #[test]
    fn a_failed_natural_draw_leaves_the_life_untouched() {
        use lifescape_planner::upkeep_role::{operator_assigned, operator_step, Assigned, OperatorStep, Switch};
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        let pinned = lifescape_planner::pinned_ambition("smithing", 99, &now, 11, &[], stamp).expect("pin");
        let mut owen = life_holding("t_natural_fail", pinned);
        let mut node = lifescape_planner::Node::new("n1", lifescape_planner::NodeKind::ReachLevel { skill: "smithing".to_string(), level: 5 }, Vec::new(), 5);
        node.status = lifescape_planner::NodeStatus::Ready;
        owen.nodes.push(node);
        let before = owen.clone();
        let OperatorStep::Assign { draft, switch } = operator_step(&owen, super::NATURAL) else { panic!("natural is given on a draft") };
        assert_eq!(switch, Switch::Natural);
        assert!(matches!(operator_assigned(&mut owen, *draft, &switch, false, 7), Assigned::Refused(_)));
        assert_eq!(owen, before);
    }

    /// The pin leaves, the ambition is given by value (A4) and the
    /// revision is `operator:natural`; the bot is then ordinary (no pin,
    /// not an operator ambition, drawn by need).
    #[test]
    fn natural_unpins_and_gives_the_ambition_by_value() {
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        let pinned = lifescape_planner::pinned_ambition("smithing", 99, &now, 11, &[], stamp).expect("pinned smithing");
        assert!(lifescape_planner::pinned(&pinned), "precondition");
        let mut owen = life_holding("t_natural_pin", pinned);
        super::request("t_natural_pin", "natural").expect("filed");
        assert!(super::apply_pending(&mut owen, &live()));
        let held = owen.ambition.as_ref().expect("an ambition");
        assert!(held.pin.is_none(), "pin removed");
        assert!(!lifescape_planner::pinned(held), "an ordinary ambition now");
        assert_ne!(held.draw_rule, lifescape_planner::OPERATOR_RULE);
        let last = held.revisions.last().expect("a revision");
        assert_eq!(last.why, "operator:natural");
        assert_eq!(last.from, "smith");
    }

    /// Ryan Mason's case: the alchemist type leaves for a trade by value.
    #[test]
    fn natural_takes_the_operator_type_away() {
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        let mut ryan = life_holding("t_natural_alcher", lifescape_planner::new_ambition(super::ALCHER, "operator:alcher", &now, 11, &[], stamp).expect("alcher"));
        super::request("t_natural_alcher", "natural").expect("filed");
        assert!(super::apply_pending(&mut ryan, &live()));
        let held = ryan.ambition.as_ref().expect("an ambition");
        assert_ne!(held.kind, super::ALCHER);
        assert_eq!(held.revisions.last().map(|revision| revision.why.as_str()), Some("operator:natural"));
    }

    /// User 2026-11-20: the retired makers are placed on trades by the
    /// operator; the market maker and unknown kinds stay refused.
    #[test]
    fn an_operator_hands_out_alcher_or_any_drawn_trade() {
        for kind in ["alcher", "cook", "fisher", "crafter", "firemaker", "priest", "runecrafter"] {
            assert!(super::request("t_operator", kind).is_ok(), "{kind}");
        }
        assert!(super::request("t_operator", lifescape_planner::MARKET_MAKER_KIND).is_err());
        assert!(super::request("t_operator", "pirate").is_err());
        assert!(super::request("", "cook").is_err());
    }
}
