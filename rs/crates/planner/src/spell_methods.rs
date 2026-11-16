//! Spell methods D18 adds (user 2026-11-16: every F2P spell trains
//! magic, no exception), beside the fights, enchants, alchemy, superheat
//! and telegrab already planned. One action = one cast (plan-rendements
//! §7, unit S): the dump's XP per cast, the dump's runes per cast.
//!
//! - `teleport:<spell>`: Varrock (25), Lumbridge (31), Falador (37),
//!   cast anywhere (`teleport.rs2`: lands within 2 tiles of the city);
//! - `curse:<spell>/<spot>`: confuse (3), weaken (11), curse (19), bind
//!   (20) on the NPCs of a combat spot (`player_magic.rs2`: XP at the
//!   cast, splash or not; a foe whose stat is already lowered, or held,
//!   refuses the cast);
//! - `bananas:bones`: bones to bananas (15), one cast converts every
//!   carried bone (`convert_bones.rs2`, a spellbook button);
//! - crumble undead (39) rides the fights (`fight:magic/crumble_undead/
//!   <spot>`) on spots whose every NPC carries the server's `undead`
//!   param (`gamedata::spells::affects`).
//!
//! The sets come from the spell dump, never a hand list: the F2P NPC
//! debuffs are the combat-spellbook spells without a max hit, the
//! teleports the F2P `*_teleport` rows.

use lifescape_gamedata::obtain::{Way, WayKind};
use lifescape_gamedata::spells::Spell;

/// The spell `bananas:` casts.
pub const BONES_TO_BANANAS: &str = "bones_to_bananas";
/// The only bones it converts (`convert_bones.rs2`: `inv_total(inv, bones)`).
pub const BANANA_BONES: &str = "bones";
/// The fruit it makes, one per bone.
pub const BANANA: &str = "banana";
/// Bones one cast converts: the pack beside its three rune stacks
/// (`training::LOAD_SLOTS` less three; the script fills the pack).
pub const BANANA_BONES_PER_CAST: i64 = 24;

fn f2p_spell(spell: &str) -> Option<Spell> {
    lifescape_gamedata::spells::all_spells().into_iter().find(|row| row.spell == spell && !row.members && row.level >= 1)
}

fn sorted_by_level(mut rows: Vec<Spell>) -> Vec<String> {
    rows.sort_by(|left, right| left.level.cmp(&right.level).then_with(|| left.spell.cmp(&right.spell)));
    rows.into_iter().map(|row| row.spell).collect()
}

/// F2P teleports, lowest level first (Varrock, Lumbridge, Falador).
pub fn teleport_spells() -> Vec<String> {
    sorted_by_level(lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.level >= 1 && row.spell.ends_with("_teleport") && !row.runes.is_empty()).collect())
}

/// F2P NPC debuffs, lowest level first (confuse, weaken, curse, bind).
pub fn debuff_spells() -> Vec<String> {
    sorted_by_level(lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.npc_debuff()).collect())
}

/// True when `spell` affects every NPC of the combat spot `spot` (crumble
/// undead: all undead; any other spell: any spot).
pub fn spell_fits_spot(spell: &str, spot: &str) -> bool {
    lifescape_gamedata::combat_spots::by_key(spot).is_some_and(|found| found.npc_names.iter().all(|npc| lifescape_gamedata::spells::affects(spell, npc)))
}

/// Combat spots a magic run casts `spell` on, weakest first: the spell
/// fits them and they stand on the surface (the magic hunt never walks
/// a bare key down, `kit::hunt::run_spot_with`).
pub fn spell_spots(spell: &str) -> Vec<&'static str> {
    lifescape_gamedata::combat_spots::catalogue_spots().into_iter().filter(|spot| spot.entrance.is_none() && spell_fits_spot(spell, spot.key)).map(|spot| spot.key).collect()
}

/// Spell one action of a per-cast method casts: fights (`fight:magic/`),
/// curses, teleports, bananas. `None` for anything else.
pub fn cast_spell(method: &str) -> Option<String> {
    if method.starts_with("fight:magic/") {
        return crate::method_way::magic_spell(method);
    }
    if let Some(tail) = method.strip_prefix("curse:") {
        let (spell, _) = tail.split_once('/')?;
        return debuff_spells().into_iter().find(|known| known == spell);
    }
    if let Some(spell) = method.strip_prefix("teleport:") {
        return teleport_spells().into_iter().find(|known| known == spell);
    }
    (method == format!("bananas:{BANANA_BONES}")).then(|| BONES_TO_BANANAS.to_string())
}

/// One cast of `spell`: magic at its level, its runes consumed.
fn cast_way(spell: &str, detail: String) -> Option<Way> {
    let row = f2p_spell(spell)?;
    Some(Way {
        kind: WayKind::Spell,
        detail,
        skill: Some(("magic".to_string(), row.level)),
        inputs: row.runes.iter().map(|(rune, count)| (rune.clone(), *count)).collect(),
        tools: Vec::new(),
        coins: 0,
        rate: None,
        qty: None,
        combat: None,
        quest: None,
        sites: Vec::new(),
        file: String::new(),
        success: 1.0,
    })
}

/// Way of a D18 spell method and the object it yields (`""` for the
/// casts that yield none: teleports, curses). `None` for other methods.
pub fn spell_way(method: &str) -> Option<(String, Way)> {
    if let Some(spell) = method.strip_prefix("teleport:") {
        cast_spell(method)?;
        return Some((String::new(), cast_way(spell, spell.to_string())?));
    }
    if let Some(tail) = method.strip_prefix("curse:") {
        let spell = cast_spell(method)?;
        let (_, spot) = tail.split_once('/')?;
        lifescape_gamedata::combat_spots::by_key(spot)?;
        let mut way = cast_way(&spell, format!("{spell} {spot}"))?;
        way.sites = lifescape_gamedata::combat_spots::positions(spot);
        return Some((String::new(), way));
    }
    if cast_spell(method).as_deref() == Some(BONES_TO_BANANAS) {
        let mut way = cast_way(BONES_TO_BANANAS, BONES_TO_BANANAS.to_string())?;
        way.inputs.push((BANANA_BONES.to_string(), BANANA_BONES_PER_CAST));
        way.qty = Some((BANANA_BONES_PER_CAST, BANANA_BONES_PER_CAST));
        return Some((BANANA.to_string(), way));
    }
    None
}

/// The per-cast way of any spell method, fights included: a magic
/// fight (`fight:magic/<spell>/<spot>`) burns its spell's runes per cast
/// on the spot's NPCs (check-method stages it; `method_way` keeps fights
/// out, their XP being a damage model, `sizing`).
pub fn cast_method_way(method: &str) -> Option<(String, Way)> {
    if let Some(found) = spell_way(method) {
        return Some(found);
    }
    let spell = crate::method_way::magic_spell(method)?;
    let spot = crate::method_way::fight_spot(method)?;
    let mut way = cast_way(&spell, format!("{spell} {spot}"))?;
    way.sites = lifescape_gamedata::combat_spots::positions(spot);
    Some((String::new(), way))
}

/// XP one cast of a D18 method pays: the dump's tenths over 10, whatever
/// the bones carried or the curse's roll (`give_spell_xp` at the cast).
pub fn cast_xp(method: &str) -> Option<f64> {
    let spell = cast_spell(method)?;
    let row = f2p_spell(&spell)?;
    (row.xp > 0).then_some(row.xp as f64 / 10.0)
}

/// Spot a debuff climb casts on at `level`: the magic tranche's own
/// harmless camp (`tranches::tranche_resource`: chickens, then cows),
/// whose retaliation never threatens the caster.
pub fn curse_spot(level: u8) -> String {
    crate::tranches::tranche_resource("magic", level)
}

/// Curse method the planner prices at `level` for `spell`.
pub fn curse_method(spell: &str, level: u8) -> String {
    format!("curse:{spell}/{}", curse_spot(level))
}

/// Every D18 method the planner may launch: each teleport, each debuff
/// on the camp its level casts on, bananas, crumble undead on each undead
/// surface spot (check-method lists them beside the obtain legs).
pub fn all_spell_methods() -> Vec<String> {
    let mut out: Vec<String> = teleport_spells().into_iter().map(|spell| format!("teleport:{spell}")).collect();
    for spell in debuff_spells() {
        let level = f2p_spell(&spell).map(|row| u8::try_from(row.level.clamp(1, 99)).unwrap_or(99)).unwrap_or(1);
        out.push(curse_method(&spell, level));
    }
    out.push(format!("bananas:{BANANA_BONES}"));
    for spell in lifescape_gamedata::spells::all_spells().into_iter().filter(|row| !row.members && row.deals_damage() && lifescape_gamedata::spells::target_param(&row.spell).is_some()) {
        out.extend(spell_spots(&spell.spell).into_iter().map(|spot| format!("fight:magic/{}/{spot}", spell.spell)));
    }
    out
}

#[cfg(test)]
#[path = "spell_methods_tests.rs"]
mod tests;
