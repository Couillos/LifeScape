//! Context a task report carries beside its flows (plan rendements R1,
//! D2): the planner's fine method, the site, the target, the tool or
//! weapon tier, the combat style and every skill's level at the start.
//!
//! The fine method rides the launch args as `--method <method>` (brain
//! launch, filler, check-method): the session strips it before the
//! script parses its CLI, so no script knows about it. A run launched
//! without it (operator `run`, an older daemon) says so explicitly:
//! `method_source = "none"`, empty method, nothing guessed.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

/// Launch flag carrying the planner method (session-only, stripped).
pub const METHOD_FLAG: &str = "--method";

/// Splits the session-only flags off the launch args: (script args,
/// planner method when carried).
pub fn strip_meta(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut kept = Vec::with_capacity(args.len());
    let mut method = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == METHOD_FLAG {
            if let Some(value) = args.get(index + 1).filter(|value| !value.starts_with("--")) {
                method = Some(value.clone());
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        kept.push(args[index].clone());
        index += 1;
    }
    (kept, method)
}

/// Run context frozen at the start of a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunMeta {
    /// Planner method (`gather:fishing/raw_shrimp`), empty when not carried.
    pub planner_method: String,
    /// `node` when the launch carried the method, `none` otherwise.
    pub method_source: &'static str,
    /// `--site x,z,level` of the launch.
    pub site: Option<(i32, i32, i32)>,
    /// The resource, spot or recipe the script was told to work (its
    /// first positional argument), empty when none.
    pub target: String,
    /// Best tool of the method's family owned at the start, or the worn
    /// weapon (and ammo) of a fight; empty when the method needs none.
    pub tool: String,
    /// `melee:<trained skill>`, `ranged`, `magic` for fights, else empty.
    pub combat_style: String,
    /// Every skill's level at the start (stat id, level).
    pub levels_start: Vec<(u8, u8)>,
}

/// Fight scripts (their tool is the worn weapon).
fn fight_script(script: &str) -> bool {
    matches!(script, "melee" | "ranged" | "magic")
}

/// Tier order of the metal tools and weapons, lowest first.
const TIERS: [&str; 8] = ["bronze", "iron", "steel", "black", "mithril", "adamant", "rune", "dragon"];

fn tier_of(name: &str) -> Option<usize> {
    TIERS.iter().position(|tier| name.starts_with(&format!("{tier}_")))
}

/// Family of a tool name: its name without the metal prefix
/// (`bronze_pickaxe` -> `pickaxe`).
fn family(name: &str) -> &str {
    match tier_of(name) {
        Some(index) => &name[TIERS[index].len() + 1..],
        None => name,
    }
}

/// Best held item of each family the method's tools name, by tier.
pub fn best_tools(method: &str, held: &[String]) -> Vec<String> {
    let Some(need) = lifescape_planner::tools::required_tool(method) else { return Vec::new() };
    let mut found: Vec<String> = Vec::new();
    for wanted in need.tools {
        let wanted_family = family(&wanted).to_string();
        let best = held.iter().filter(|name| family(name) == wanted_family).max_by_key(|name| tier_of(name).map(|tier| tier + 1).unwrap_or(0));
        if let Some(best) = best {
            if !found.contains(best) {
                found.push(best.clone());
            }
        }
    }
    found
}

fn debug_name(id: u16) -> Option<String> {
    lifescape_gamedata::obj_by_id(usize::from(id)).and_then(|entry| entry.name).map(str::to_string)
}

/// Fight scripts wield their kit after the start: the report reads the
/// weapon (and ammo) worn at the end, the start's when nothing is worn
/// any more (a death).
pub fn end_fight_tool(bot: &Bot, script: &str, start: &str) -> String {
    if !fight_script(script) {
        return start.to_string();
    }
    let worn = worn_fight_kit(bot).join("+");
    if worn.is_empty() { start.to_string() } else { worn }
}

/// Fight scripts pick their attack style after the start (the melee
/// kit sets the style it trains): the report reads the style at the end,
/// the start's when none is readable (live 2026-11-16: `melee:attack`
/// reports whose XP all went to strength, the style read before the
/// script set it).
pub fn end_combat_style(bot: &Bot, script: &str, start: &str) -> String {
    if script != "melee" {
        return start.to_string();
    }
    lifescape_scripts::kit::style::trained_skill(bot).map(|skill| format!("melee:{skill}")).unwrap_or_else(|| start.to_string())
}

/// Worn objects by debug name, weapon (slot 3) and ammo (slot 13) first.
fn worn_fight_kit(bot: &Bot) -> Vec<String> {
    let Some(worn) = (bot.tables.com_id)("wornitems:wear").and_then(|com| bot.world.inv.get(&com)) else { return Vec::new() };
    [3u8, 13u8].iter().filter_map(|slot| worn.get(slot).and_then(|(id, _)| debug_name(*id))).collect()
}

/// Everything the bot owns at the start (pack, worn and the last known
/// bank: the script withdraws its tool), by debug name.
fn owned_names(bot: &Bot) -> Vec<String> {
    let bank = (bot.tables.com_id)("bank_main:inv");
    let banked = bank.and_then(|com| bot.world.inv_known(com));
    let mut names: Vec<String> = bot
        .world
        .inv
        .iter()
        .filter(|(com, _)| Some(**com) != bank)
        .map(|(_, slots)| slots)
        .chain(banked)
        .flat_map(|slots| slots.values().filter_map(|(id, _)| debug_name(*id)))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Run context at the start of `script` launched with `args` (script
/// args, meta stripped) and the carried `method`.
pub fn run_meta(bot: &Bot, script: &str, args: &[String], method: Option<&str>) -> RunMeta {
    let site = args.iter().position(|arg| arg == "--site").and_then(|at| args.get(at + 1)).and_then(|raw| {
        let parts: Vec<i32> = raw.split(',').filter_map(|part| part.trim().parse().ok()).collect();
        (parts.len() == 3).then(|| (parts[0], parts[1], parts[2]))
    });
    let target = args.first().filter(|first| !first.starts_with("--")).cloned().unwrap_or_default();
    let tool = if fight_script(script) {
        worn_fight_kit(bot).join("+")
    } else {
        method.map(|method| best_tools(method, &owned_names(bot)).join("+")).unwrap_or_default()
    };
    let combat_style = match script {
        "melee" => lifescape_scripts::kit::style::trained_skill(bot).map(|skill| format!("melee:{skill}")).unwrap_or_else(|| "melee".to_string()),
        "ranged" | "magic" => script.to_string(),
        _ => String::new(),
    };
    let mut levels_start: Vec<(u8, u8)> = bot.world.stats.iter().map(|(stat, (level, _))| (*stat, *level)).collect();
    levels_start.sort_unstable();
    RunMeta {
        planner_method: method.unwrap_or_default().to_string(),
        method_source: if method.is_some() { "node" } else { "none" },
        site,
        target,
        tool,
        combat_style,
        levels_start,
    }
}

#[cfg(test)]
mod tests {
    use super::{best_tools, strip_meta};

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// The method flag leaves the script args untouched otherwise.
    #[test]
    fn method_flag_is_stripped() {
        let (kept, method) = strip_meta(&strings(&["raw_shrimp", "--method", "gather:fishing/raw_shrimp", "--node", "n4"]));
        assert_eq!(kept, strings(&["raw_shrimp", "--node", "n4"]));
        assert_eq!(method.as_deref(), Some("gather:fishing/raw_shrimp"));
        let (kept, method) = strip_meta(&strings(&["logs", "--method"]));
        assert_eq!((kept, method), (strings(&["logs"]), None), "a bare flag carries nothing");
    }

    /// The best tier of the method's tool family wins among held tools.
    #[test]
    fn best_tool_tier_wins() {
        let held = strings(&["bronze_pickaxe", "rune_pickaxe", "iron_pickaxe", "bronze_axe", "hammer"]);
        assert_eq!(best_tools("gather:mining/copper_ore", &held), strings(&["rune_pickaxe"]));
        assert_eq!(best_tools("gather:woodcut/logs", &held), strings(&["bronze_axe"]));
    }
}
