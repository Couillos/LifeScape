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

use lifescape_json::Value;
use lifescape_planner::{Life, ALCHER};

fn pending() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kinds an operator may hand out by this command.
pub const OPERATOR_KINDS: &[&str] = &[ALCHER];

/// `ambition <compte> natural` (user 2026-11-21): the operator account
/// joins the natural balance: pin or operator type removed, the ambition
/// given by the A4 value like any bot's, then ordinary (A2).
pub const NATURAL: &str = "natural";

/// A trade the fleet draws (catalogue kind with a ladder, never the
/// market maker): an operator may hand it out too.
fn drawable_trade(kind: &str) -> bool {
    lifescape_planner::ambition_catalog::specs().iter().any(|entry| entry.kind == kind) && !lifescape_planner::ambition::ladderless(kind) && kind != lifescape_planner::MARKET_MAKER_KIND
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

/// Apply the switch filed for this life, if any, and lift the bans a
/// switch caused (ryan_mason 2026-11-18: its abandoned nodes read as a
/// stuck graph, magic banned 6000 ticks); true when the life changed
/// (the caller saves it).
pub(crate) fn apply_pending(life: &mut Life, live: &Value) -> bool {
    let lifted = lift_role_bans(life);
    let Some(kind) = pending().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&life.account) else { return lifted };
    if lifescape_planner::protected(&life.account) {
        crate::logln!("brain {}: operator ambition {kind} dropped (protected account, overall for good)", life.account);
        return lifted;
    }
    if kind == NATURAL {
        return apply_natural(life, live) || lifted;
    }
    if kind != ALCHER {
        if !drawable_trade(&kind) {
            return lifted;
        }
        // A trade: the open work stops, the kind is built like a redraw
        // (its ladder over the fleet's demand and surplus), the next goal
        // drawn at once.
        let mut draft = life.clone();
        draft.goal_origin = None;
        if !crate::brain_ambition::assign_kind(&mut draft, live, &kind, &format!("operator:{kind}")) {
            crate::logln!("brain {}: operator ambition {kind} not given (nothing buildable or refused), life untouched", life.account);
            return lifted;
        }
        *life = draft;
        let abandoned = lifescape_planner::abandon_for_role(life, lifescape_planner::OPERATOR_ROLE_CAUSE);
        crate::brain::push_history(life, "goal", &format!("role:{kind} (operator)"));
        crate::logln!("brain {}: ambition -> {kind} (operator), {} nodes abandoned", life.account, abandoned.len());
        if let Some(label) = crate::brain_succession::draw_next(life, live) {
            crate::logln!("brain {}: {kind}'s first goal {label}", life.account);
        }
        lifescape_planner::normalize(life);
        return true;
    }
    let stamp = crate::brain_ambition::stamp_now();
    let switched = crate::brain_ambition::with_now(live, |now| lifescape_planner::become_alcher(life, now, stamp));
    match switched {
        Ok(abandoned) => {
            crate::brain::push_history(life, "goal", "role:alcher (operator)");
            crate::logln!("brain {}: ambition -> alcher (operator), {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(","));
            // The next goal at once (alchemy first), like the market
            // maker's switch: never a stuck graph that bans its skills.
            match crate::brain_succession::draw_next(life, live) {
                Some(label) => crate::logln!("brain {}: alcher's first goal {label}", life.account),
                None => crate::logln!("brain {}: alcher drew no goal yet (see the reason above)", life.account),
            }
            lifescape_planner::normalize(life);
            true
        }
        Err(error) => {
            crate::logln!("brain {}: operator ambition alcher refused: {error:?}", life.account);
            lifted
        }
    }
}

/// `natural`: the pin (or the alchemist type) leaves, the open work of the
/// old role stops, the ambition is given by value over the fleet's demand
/// and the tenants already on each kind (`brain_ambition::assign_natural`,
/// revision `operator:natural`), the next goal drawn at once. Refused
/// (logged) for a protected account or a market maker.
fn apply_natural(life: &mut Life, live: &Value) -> bool {
    apply_natural_with(life, live, &mut |draft| crate::brain_ambition::assign_natural(draft, live))
}

/// `apply_natural` with the giving step injected. The work is done on a
/// copy: the life changes (pin, rule, nodes, history) only when the step
/// succeeded; a failed draw leaves it exactly as it was.
fn apply_natural_with(life: &mut Life, live: &Value, assign: &mut dyn FnMut(&mut Life) -> bool) -> bool {
    if lifescape_planner::protected(&life.account) || lifescape_planner::maker_role(life).is_some() {
        crate::logln!("brain {}: operator ambition natural refused (protected account or market maker)", life.account);
        return false;
    }
    let mut draft = life.clone();
    if let Some(held) = draft.ambition.as_mut() {
        held.pin = None;
        held.draw_rule.clear();
    }
    draft.goal_origin = None;
    if !assign(&mut draft) {
        crate::logln!("brain {}: natural balance not given (no draw or change refused), life untouched", life.account);
        return false;
    }
    *life = draft;
    let abandoned = lifescape_planner::abandon_for_role(life, lifescape_planner::OPERATOR_ROLE_CAUSE);
    crate::brain::push_history(life, "goal", "role:natural (operator)");
    crate::logln!("brain {}: ambition -> natural balance (operator), {} nodes abandoned", life.account, abandoned.len());
    if let Some(label) = crate::brain_succession::draw_next(life, live) {
        crate::logln!("brain {}: first goal {label}", life.account);
    }
    lifescape_planner::normalize(life);
    true
}

/// Unban the skills a role switch banned (`planner::role_lifted_bans`).
fn lift_role_bans(life: &mut Life) -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    let skills = lifescape_planner::role_lifted_bans(&life.history, &ticks_since);
    for skill in &skills {
        crate::brain::push_history(life, "goal", &lifescape_planner::unban_entry(skill));
        crate::logln!("brain {}: unban:{skill} (an operator role switch caused it)", life.account);
    }
    !skills.is_empty()
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
        let now = lifescape_planner::BotNow { level_of: &|_| 1, coins: 0, held: &|_| 0 };
        let stamp = lifescape_planner::Stamp { tick: 0, secs: 0 };
        let pinned = lifescape_planner::pinned_ambition("smithing", 99, &now, 11, &[], stamp).expect("pin");
        let mut owen = life_holding("t_natural_fail", pinned);
        let mut node = lifescape_planner::Node::new("n1", lifescape_planner::NodeKind::ReachLevel { skill: "smithing".to_string(), level: 5 }, Vec::new(), 5);
        node.status = lifescape_planner::NodeStatus::Ready;
        owen.nodes.push(node);
        let before = owen.clone();
        assert!(!super::apply_natural_with(&mut owen, &live(), &mut |_| false));
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
