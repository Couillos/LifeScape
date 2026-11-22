//! Spell loads: stock the pack for a run of casts from the known bank.
//! A cast's runes and targets count in the pack AND the last known bank
//! view (`banked_usable`): a cast the bank funds is a bank trip, never a
//! `blocked` (live 2026-11-13: bd23dqpq blocked four times "missing:
//! cosmicrune, waterrune" on enchant_lvl1 with 2 cosmic and 67 water
//! banked, the pack-only check never opened the bank). An elemental staff
//! pays its rune like the planner counts it (`tools::family_held`): worn
//! as is, carried or banked wielded before the load is sized.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{held_total, poll_stop, CycleState, StopReason, TaskSpec};
use crate::kit::magic::{staff_pays, wield_staff_for, STAVES};
use crate::kit::pack::{banked_usable, free_slots, obj_id, pack_total};
use crate::ScriptError;

/// What one cast eats: runes (dump, per cast) and inputs (per cast, the
/// first is the cast target; none for teleports and telegrab).
/// `whole_pack`: one cast converts every carried target (bones to
/// bananas), so the runes ride for one cast and the inputs fill the pack.
/// `noted_target`: the cast takes its target as a banknote too (alchemy:
/// `oc_cost` of a `cert_` obj is its base cost, `inv_del` eats one note),
/// so the target comes out of the bank noted, one slot for the whole load.
#[derive(Debug, Clone)]
pub struct SpellLoad {
    pub spell: String,
    pub runes: Vec<(String, i64)>,
    pub inputs: Vec<(String, u32)>,
    pub whole_pack: bool,
    pub noted_target: bool,
}

/// One carried need: name, units per cast, rune (sized per cast even in
/// a whole-pack load) or input, and whether its banknote counts and is
/// withdrawn (the noted target).
struct Need {
    name: String,
    per_cast: u32,
    rune: bool,
    noted: bool,
}

/// Banknote id of `base` (`cert_<name>`), `None` when it has none.
pub fn note_of(base: u16) -> Option<u16> {
    let note = crate::market_client::trade_id(base);
    (note != base).then_some(note)
}

/// Pack units of a need: its banknotes too when the need rides noted.
fn carried_units(bot: &Bot, obj: u16, noted: bool) -> u32 {
    let notes = if noted { note_of(obj).map(|note| pack_total(bot, note)).unwrap_or(0) } else { 0 };
    pack_total(bot, obj).saturating_add(notes)
}

/// Where a staff paying one of the cast's runes sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaffAt {
    Worn,
    Pack,
    Bank,
}

/// The staff the load relies on: the worn one paying a cast rune, else the
/// first carried, else the first banked one (STAVES order, the same order
/// `wield_staff_for` picks). (staff, rune it pays, where).
pub fn staff_choice(bot: &Bot, runes: &[(String, i64)]) -> Option<(&'static str, &'static str, StaffAt)> {
    let cast_runes = STAVES.iter().filter(|(_, rune)| runes.iter().any(|(name, _)| name == rune));
    let worn = cast_runes.clone().find(|(_, rune)| staff_pays(bot, rune)).map(|(staff, rune)| (*staff, *rune, StaffAt::Worn));
    let held = |at: StaffAt| {
        cast_runes.clone().find(|(staff, _)| {
            obj_id(bot, staff).is_ok_and(|obj| if at == StaffAt::Pack { pack_total(bot, obj) > 0 } else { banked_usable(bot, obj) > 0 })
        })
        .map(|(staff, rune)| (*staff, *rune, at))
    };
    worn.or_else(|| held(StaffAt::Pack)).or_else(|| held(StaffAt::Bank))
}

impl SpellLoad {
    pub fn new(spell: &str, runes: Vec<(String, i64)>, inputs: Vec<(String, u32)>) -> SpellLoad {
        SpellLoad { spell: spell.to_string(), runes, inputs, whole_pack: false, noted_target: false }
    }

    /// Carried needs with the rune `paid` by a staff left out.
    fn needs(&self, paid: Option<&str>) -> Vec<Need> {
        let runes = self.runes.iter().filter(|(name, _)| paid != Some(name.as_str())).map(|(name, count)| Need {
            name: name.clone(),
            per_cast: u32::try_from(*count).unwrap_or(u32::MAX).max(1),
            rune: true,
            noted: false,
        });
        let inputs = self.inputs.iter().enumerate().map(|(at, (name, count))| Need { name: name.clone(), per_cast: (*count).max(1), rune: false, noted: at == 0 && self.noted_target });
        runes.chain(inputs).collect()
    }

    /// Rune a WORN staff pays now (the server's view at cast time).
    fn worn_paid(&self, bot: &Bot) -> Option<&str> {
        self.runes.iter().map(|(name, _)| name.as_str()).find(|name| staff_pays(bot, name))
    }

    /// Item ids the load carries (runes and inputs, all of them): kept
    /// through the deposit that banks the products.
    pub fn carried_ids(&self, bot: &Bot) -> Vec<u16> {
        let mut ids = Vec::new();
        for need in self.needs(None) {
            let Ok(obj) = obj_id(bot, &need.name) else { continue };
            ids.push(obj);
            if let Some(note) = need.noted.then(|| note_of(obj)).flatten() {
                ids.push(note);
            }
        }
        ids
    }

    /// Casts the pack funds right now (worn staff counted); `u32::MAX`
    /// when the cast eats nothing at all.
    pub fn pack_casts(&self, bot: &Bot) -> u32 {
        self.needs(self.worn_paid(bot))
            .iter()
            .map(|need| obj_id(bot, &need.name).map(|obj| carried_units(bot, obj, need.noted) / need.per_cast).unwrap_or(0))
            .min()
            .unwrap_or(u32::MAX)
    }

    /// Exact names pack + known bank cannot fund for ONE cast, a worn,
    /// carried or banked staff paying its rune: the `blocked` list.
    pub fn shortfall(&self, bot: &Bot) -> Vec<String> {
        let paid = staff_choice(bot, &self.runes).map(|(_, rune, _)| rune);
        self.needs(paid)
            .into_iter()
            .filter(|need| obj_id(bot, &need.name).map(|obj| carried_units(bot, obj, need.noted).saturating_add(banked_usable(bot, obj)) < need.per_cast).unwrap_or(true))
            .map(|need| need.name)
            .collect()
    }

    /// Withdrawals (obj, units) that stock the pack for up to `wanted`
    /// casts (whole-pack loads: `wanted` targets, runes for one cast):
    /// bounded by pack + bank and by free slots (a stack takes one slot
    /// once, a non-stackable one per unit). A noted target is withdrawn
    /// as its banknote id (one slot). Empty when nothing fits.
    pub fn plan(&self, bot: &Bot, wanted: u32) -> Vec<(u16, u32)> {
        let free = u64::try_from(free_slots(bot)).unwrap_or(0);
        let rows: Vec<(u16, u64, u64, u64, bool, bool, u64)> = self
            .needs(self.worn_paid(bot))
            .iter()
            .filter_map(|need| {
                let obj = obj_id(bot, &need.name).ok()?;
                let note = need.noted.then(|| note_of(obj)).flatten();
                let stackable = note.is_some() || knowledge::obj_by_id(usize::from(obj)).is_some_and(|entry| entry.stackable);
                let have = u64::from(carried_units(bot, obj, need.noted));
                // A pack already holding the base form keeps its slots;
                // the notes take one more only when none ride yet.
                let pulled = note.unwrap_or(obj);
                let stack_have = if note.is_some() { u64::from(pack_total(bot, pulled)) } else { have };
                Some((pulled, u64::from(need.per_cast), have, u64::from(banked_usable(bot, obj)), stackable, need.rune, stack_have))
            })
            .collect();
        let whole = self.whole_pack;
        let units = |per: u64, rune: bool, casts: u64| if whole && rune { per * casts.min(1) } else { per * casts };
        let slots_for = |casts: u64| -> u64 {
            rows.iter()
                .map(|(_, per, have, _, stackable, rune, stack_have)| {
                    let extra = units(*per, *rune, casts).saturating_sub(*have);
                    if *stackable { u64::from(extra > 0 && *stack_have == 0) } else { extra }
                })
                .sum()
        };
        let mut top = u64::from(wanted);
        for (_, per, have, banked, _, rune, _) in &rows {
            if !(whole && *rune) {
                top = top.min((have + banked) / per);
            } else if have + banked < *per {
                top = 0;
            }
        }
        // Largest cast count whose withdrawals fit (slots grow with casts).
        let (mut low, mut high) = (0u64, top);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if slots_for(middle) <= free { low = middle } else { high = middle - 1 }
        }
        rows.iter()
            .map(|(obj, per, have, _, _, rune, _)| (*obj, units(*per, *rune, low).saturating_sub(*have)))
            .filter(|(_, count)| *count > 0)
            .map(|(obj, count)| (obj, u32::try_from(count).unwrap_or(u32::MAX)))
            .collect()
    }
}

/// Wield the staff the load relies on (`staff_choice`): carried goes on
/// at once, banked comes out of the bank first. True when one went on.
pub fn ensure_staff(bot: &mut Bot, bank: &knowledge::banks::Bank, runes: &[(String, i64)]) -> Result<bool, ScriptError> {
    match staff_choice(bot, runes) {
        Some((_, _, StaffAt::Pack)) => wield_staff_for(bot, runes),
        Some((staff, rune, StaffAt::Bank)) => {
            let obj = obj_id(bot, staff)?;
            if crate::kit::withdraw::bank_withdraw_items(bot, bank, &[obj], 1)? > 0 && crate::kit::gear::wield_held(bot, obj)? {
                bot.log(&format!("wielding banked {staff} for the {rune} of the cast"));
                return Ok(true);
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Stock the pack from the bank: the staff first, products banked (the
/// load's own runes and inputs kept), then the plan for `wanted` casts in
/// one booth visit. `Blocked` names exactly what pack + bank lack.
pub fn refill(bot: &mut Bot, bank: &knowledge::banks::Bank, load: &SpellLoad, keep: &[u16], wanted: u32) -> Result<u32, ScriptError> {
    let missing = load.shortfall(bot);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    wield_staff_for(bot, &load.runes)?;
    if load.pack_casts(bot) > 0 {
        return Ok(load.pack_casts(bot));
    }
    let mut kept = keep.to_vec();
    kept.extend(load.carried_ids(bot));
    kept.extend(STAVES.iter().filter_map(|(staff, _)| obj_id(bot, staff).ok()));
    crate::kit::bank::bank_trip(bot, bank, &kept, 0)?;
    // Products banked first: a banked staff then has a slot to land in.
    ensure_staff(bot, bank, &load.runes)?;
    let plan = load.plan(bot, wanted.max(1));
    bot.log(&format!("{} load from the bank: {}", load.spell, plan.iter().map(|(obj, count)| format!("{} x{count}", crate::kit::pack::obj_label(bot, *obj))).collect::<Vec<_>>().join(", ")));
    crate::kit::withdraw::bank_withdraw_counts(bot, bank, &plan)?;
    let casts = load.pack_casts(bot);
    if casts > 0 {
        return Ok(casts);
    }
    let missing = load.shortfall(bot);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    Err(ScriptError::Failed(format!("{} load never reached the pack at {:?} ({} free slots)", load.spell, bot.position(), free_slots(bot))))
}

/// Pack funds a cast, or a bank refill makes it so (single casts:
/// teleports, telegrab). Short in pack + bank blocks by exact names
/// before any walk.
pub fn stock_for(bot: &mut Bot, load: &SpellLoad, keep: &[u16], wanted: u32) -> Result<(), ScriptError> {
    if load.pack_casts(bot) > 0 {
        return Ok(());
    }
    let missing = load.shortfall(bot);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    let bank = crate::kit::bank::nearest_bank(bot)?;
    refill(bot, &bank, load, keep, wanted).map(|_| ())
}

/// True when the order names no goal (manual "do all I hold" runs).
fn goalless(spec: &TaskSpec) -> bool {
    let targets = &spec.targets;
    targets.count.is_none() && targets.want_item.is_none() && targets.xp_skill.is_none() && targets.level_skill.is_none() && targets.uses.is_empty()
}

/// A goal run that cast and then found the runes spent ends as a spent
/// window (the brain counts the XP as progress and, at the next launch,
/// its input gate files the runes): a `Blocked` error row after real work
/// was noise (live 2026-11-22: curse/magic `missing waterrune` after 28
/// casts). No cast done: `None`, the plain `Blocked` stays.
pub fn ran_dry_window(casts_done: u32) -> Option<StopReason> {
    (casts_done > 0).then_some(StopReason::BudgetExhausted)
}

/// Casts the order still asks for (count left, `--want` left), `u32::MAX`
/// for a level/XP climb or a goalless run.
fn casts_left(bot: &Bot, spec: &TaskSpec, done: u32, want_base: Option<u32>) -> u32 {
    let by_count = spec.targets.count.map(|count| count.saturating_sub(done));
    let by_want = match (spec.targets.want_item.as_deref(), spec.targets.want_count, want_base) {
        (Some(item), Some(want), Some(base)) => Some(want.saturating_sub(held_total(bot, item).saturating_sub(base))),
        _ => None,
    };
    by_count.into_iter().chain(by_want).min().unwrap_or(u32::MAX)
}

/// What one call of a cast step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastOutcome {
    /// One cast paid its XP: a unit.
    Cast,
    /// The pack funds no cast any more (a market pause banked the runes,
    /// live 2026-11-19): no unit, the loop refills from the bank.
    Refill,
}

/// Cast loop with bank refills: the order's stops first, a refill when
/// the pack funds no cast, then `cast` once. Count 0 = no count. A
/// goalless run that ran dry after casting ends `(done, None)`; any
/// order short in pack + bank blocks by name.
pub fn run_casts(
    bot: &mut Bot,
    spec: &TaskSpec,
    load: &SpellLoad,
    keep: &[u16],
    mut cast: impl FnMut(&mut Bot) -> Result<(), ScriptError>,
) -> Result<(u32, Option<StopReason>), ScriptError> {
    run_cast_steps(bot, spec, load, keep, |cast_bot| cast(cast_bot).map(|_| CastOutcome::Cast))
}

/// `run_casts` with a step that may ask for a refill instead of a cast
/// (`CastOutcome::Refill`: no unit counted, the loop's own pack check
/// takes the runes again).
pub fn run_cast_steps(
    bot: &mut Bot,
    spec: &TaskSpec,
    load: &SpellLoad,
    keep: &[u16],
    mut cast: impl FnMut(&mut Bot) -> Result<CastOutcome, ScriptError>,
) -> Result<(u32, Option<StopReason>), ScriptError> {
    let mut spec = spec.clone();
    spec.targets.count = spec.targets.count.filter(|count| *count > 0);
    let mut state = CycleState::begin(bot, &spec);
    let want_base = spec.targets.want_item.as_deref().map(|item| held_total(bot, item));
    let mut bank: Option<knowledge::banks::Bank> = None;
    loop {
        if let Some(stop) = poll_stop(bot, &state, &spec) {
            return Ok((state.units(), Some(stop)));
        }
        if load.pack_casts(bot) == 0 {
            let missing = load.shortfall(bot);
            if !missing.is_empty() {
                if state.units() > 0 && goalless(&spec) {
                    bot.log(&format!("{} ran dry after {} casts (pack and bank): {}", load.spell, state.units(), missing.join(", ")));
                    return Ok((state.units(), None));
                }
                // The lot's stock is spent (`kit::stock_use`): done.
                if state.stock_spent(bot, &spec) {
                    bot.log(&format!("{} used up its stock after {} casts: {}", load.spell, state.units(), missing.join(", ")));
                    return Ok((state.units(), Some(StopReason::GoalReached)));
                }
                if let Some(stop) = ran_dry_window(state.units()) {
                    bot.log(&format!("{} ran dry after {} casts toward its goal (pack and bank): {}", load.spell, state.units(), missing.join(", ")));
                    return Ok((state.units(), Some(stop)));
                }
                return Err(ScriptError::Blocked { missing });
            }
            let site = match bank.take() {
                Some(known) => known,
                None => crate::kit::bank::nearest_bank(bot)?,
            };
            let wanted = casts_left(bot, &spec, state.units(), want_base);
            refill(bot, &site, load, keep, wanted)?;
            bank = Some(site);
        }
        match cast(bot)? {
            CastOutcome::Cast => state.note_unit(),
            CastOutcome::Refill => bot.log(&format!("{}: the pack funds no cast any more, refilling", load.spell)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{staff_choice, SpellLoad, StaffAt};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Offline bot with a pack and a closed bank view (what the brain
    /// reads as held: the save at login, the last booth visit).
    pub(crate) fn stocked_bot(pack: &[(&str, u32)], banked: &[(&str, u32)]) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com in dumps");
        let slots = pack.iter().enumerate().map(|(slot, (name, count))| (slot as u8, (obj_id(&bot, name).expect("pack obj"), *count))).collect();
        bot.world.inv.insert(pack_com, slots);
        bot.world.inv_cap.insert(pack_com, 28);
        let bank_com = (bot.tables.com_id)("bank_main:inv").expect("bank com in dumps");
        let counts = banked.iter().map(|(name, count)| (obj_id(&bot, name).expect("bank obj"), *count)).collect();
        bot.world.seed_closed_inv(bank_com, &counts);
        bot
    }

    fn enchant_load() -> SpellLoad {
        let runes = crate::activities::enchant_teleport::spell_runes("enchant_lvl1");
        SpellLoad::new("enchant_lvl1", runes, vec![("strung_sapphire_amulet".to_string(), 1)])
    }

    fn named(_bot: &Bot, plan: &[(u16, u32)]) -> Vec<(String, u32)> {
        let mut rows: Vec<(String, u32)> = plan.iter().map(|(obj, count)| (lifescape_gamedata::obj_by_id(usize::from(*obj)).and_then(|entry| entry.name).unwrap_or_default().to_string(), *count)).collect();
        rows.sort();
        rows
    }

    /// bd23dqpq (live 2026-11-13): 2 cosmic + 67 water banked, empty
    /// pack. Nothing short (the bank funds two casts), the pack funds
    /// none, the plan withdraws two casts' worth.
    #[test]
    fn banked_runes_fund_the_load() {
        let bot = stocked_bot(&[], &[("cosmicrune", 2), ("waterrune", 67), ("strung_sapphire_amulet", 10)]);
        let load = enchant_load();
        assert!(load.shortfall(&bot).is_empty(), "the bank funds the cast, got {:?}", load.shortfall(&bot));
        assert_eq!(load.pack_casts(&bot), 0, "pack alone funds nothing");
        let plan = named(&bot, &load.plan(&bot, u32::MAX));
        assert_eq!(plan, vec![("cosmicrune".to_string(), 2), ("strung_sapphire_amulet".to_string(), 2), ("waterrune".to_string(), 2)]);
        assert_eq!(named(&bot, &load.plan(&bot, 1)), vec![("cosmicrune".to_string(), 1), ("strung_sapphire_amulet".to_string(), 1), ("waterrune".to_string(), 1)], "the order caps the load");
    }

    /// Short in pack AND bank: exact names, nothing else.
    #[test]
    fn shortfall_names_only_what_pack_and_bank_lack() {
        let bot = stocked_bot(&[("strung_sapphire_amulet", 1)], &[("waterrune", 67)]);
        assert_eq!(enchant_load().shortfall(&bot), vec!["cosmicrune".to_string()]);
        let bot = stocked_bot(&[("cosmicrune", 1)], &[("waterrune", 67)]);
        assert_eq!(enchant_load().shortfall(&bot), vec!["strung_sapphire_amulet".to_string()]);
        let bot = stocked_bot(&[], &[]);
        assert_eq!(enchant_load().shortfall(&bot), vec!["cosmicrune".to_string(), "waterrune".to_string(), "strung_sapphire_amulet".to_string()]);
    }

    /// Pack space bounds the load: amulets take a slot each, rune stacks
    /// one slot once (here 2 junk-free slots short of a full pack).
    #[test]
    fn free_slots_bound_the_plan() {
        let pack: Vec<(&str, u32)> = vec![("bronze_dagger", 1); 20];
        let bot = stocked_bot(&pack, &[("cosmicrune", 100), ("waterrune", 100), ("strung_sapphire_amulet", 50)]);
        // 8 free: 2 rune stacks + 6 amulets.
        assert_eq!(named(&bot, &enchant_load().plan(&bot, u32::MAX)), vec![("cosmicrune".to_string(), 6), ("strung_sapphire_amulet".to_string(), 6), ("waterrune".to_string(), 6)]);
    }

    /// A staff pays its element like the planner counts it: a banked
    /// staff of water clears the water shortfall; worn, the plan skips
    /// the water runes entirely.
    #[test]
    fn staff_pays_its_rune_worn_carried_or_banked() {
        let bot = stocked_bot(&[], &[("cosmicrune", 5), ("strung_sapphire_amulet", 5), ("staff_of_water", 1)]);
        assert_eq!(staff_choice(&bot, &enchant_load().runes), Some(("staff_of_water", "waterrune", StaffAt::Bank)));
        assert!(enchant_load().shortfall(&bot).is_empty(), "banked staff pays the water");
        let mut bot = stocked_bot(&[], &[("cosmicrune", 5), ("strung_sapphire_amulet", 5)]);
        let staff = obj_id(&bot, "staff_of_water").expect("staff");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com");
        bot.world.inv.insert(worn_com, [(3u8, (staff, 1u32))].into_iter().collect());
        assert_eq!(staff_choice(&bot, &enchant_load().runes).map(|(_, _, at)| at), Some(StaffAt::Worn));
        assert_eq!(named(&bot, &enchant_load().plan(&bot, 3)), vec![("cosmicrune".to_string(), 3), ("strung_sapphire_amulet".to_string(), 3)]);
    }

    /// Bones to bananas: one cast converts the pack, so the runes ride for
    /// one cast while the bones fill the free slots.
    #[test]
    fn whole_pack_load_sizes_runes_for_one_cast() {
        let bot = stocked_bot(&[], &[("bones", 100), ("naturerune", 10), ("earthrune", 20), ("waterrune", 20)]);
        let runes = crate::activities::enchant_teleport::spell_runes("bones_to_bananas");
        let mut load = SpellLoad::new("bones_to_bananas", runes, vec![("bones".to_string(), 1)]);
        load.whole_pack = true;
        assert_eq!(named(&bot, &load.plan(&bot, u32::MAX)), vec![("bones".to_string(), 25), ("earthrune".to_string(), 2), ("naturerune".to_string(), 1), ("waterrune".to_string(), 2)]);
    }

    /// Alchemy takes its target noted: 300 banked yew logs come out as
    /// one stack of notes (the plan names the `cert_` id), never 25 per
    /// trip; carried notes count as casts and stay kept through deposits.
    #[test]
    fn a_noted_target_rides_one_slot() {
        let runes = crate::activities::alchemy::spell_runes("highlvl_alchemy");
        let mut load = SpellLoad::new("highlvl_alchemy", runes, vec![("yew_logs".to_string(), 1)]);
        load.noted_target = true;
        let bot = stocked_bot(&[], &[("yew_logs", 300), ("naturerune", 400), ("firerune", 2000)]);
        assert_eq!(named(&bot, &load.plan(&bot, u32::MAX)), vec![("cert_yew_logs".to_string(), 300), ("firerune".to_string(), 1500), ("naturerune".to_string(), 300)]);
        assert_eq!(named(&bot, &load.plan(&bot, 40)), vec![("cert_yew_logs".to_string(), 40), ("firerune".to_string(), 200), ("naturerune".to_string(), 40)], "the order caps the load");
        let bot = stocked_bot(&[("cert_yew_logs", 50), ("naturerune", 60), ("firerune", 500)], &[]);
        assert_eq!(load.pack_casts(&bot), 50, "notes are casts");
        assert!(load.shortfall(&bot).is_empty());
        let logs = obj_id(&bot, "yew_logs").expect("logs");
        let note = super::note_of(logs).expect("yew logs have a note");
        assert!(load.carried_ids(&bot).contains(&note), "the notes survive the products deposit");
        let mut plain = load.clone();
        plain.noted_target = false;
        assert_eq!(plain.pack_casts(&bot), 0, "unnoted loads read the base form only");
    }

    #[test]
    fn a_goal_run_that_cast_then_ran_dry_is_a_spent_window() {
        assert_eq!(super::ran_dry_window(28), Some(super::StopReason::BudgetExhausted));
        assert_eq!(super::ran_dry_window(0), None);
    }

    /// Lot 8 review: a market pause banks the runes mid-cast (curse: 12
    /// "You do not have enough ... Runes" refusals). The cast step says
    /// `Refill`: no unit counted, the loop's pack check runs again (here
    /// the pack still funds the cast, so the next step casts).
    #[test]
    fn a_refill_step_counts_no_unit() {
        let runes = crate::activities::enchant_teleport::spell_runes("confuse");
        let load = SpellLoad::new("confuse", runes, Vec::new());
        let mut bot = stocked_bot(&[("bodyrune", 10), ("earthrune", 20), ("waterrune", 30)], &[]);
        assert!(load.pack_casts(&bot) > 0, "funded");
        let spec = crate::kit::cycle::parse_cli("curse", &["chickens".to_string(), "2".to_string()]).expect("order");
        let mut steps = vec![super::CastOutcome::Cast, super::CastOutcome::Refill, super::CastOutcome::Cast].into_iter();
        let mut calls = 0u32;
        let (done, _) = super::run_cast_steps(&mut bot, &spec, &load, &[], |_| {
            calls += 1;
            Ok(steps.next().expect("no third unit asked"))
        })
        .expect("runs offline");
        assert_eq!((done, calls), (2, 3), "the refill step is no cast");
    }
}
