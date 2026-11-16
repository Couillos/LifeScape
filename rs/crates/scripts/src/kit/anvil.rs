//! Anvil: one smithing visit (catalogue §5.7). The bar goes on the post
//! anvil from the nearest stand that reaches, the columns open, one Make
//! press (`inv_button1/2/3` = 1/5/10 items, `smithing.rs2`) and the
//! server chains the items until the bars or the press run out. The
//! `smith` activity declares the product; this brick knows the anvil.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::pack_total;
use crate::kit::press::{follow_chain, Chain};
use crate::kit::survival::is_walk_abort;
use crate::ScriptError;

pub const HAMMER: &str = "hammer";
/// Ticks for the columns to open after the bar lands on the anvil.
const OPEN_TICKS: u64 = 20;
/// Ticks without a bar going before a Make chain reads as over (an item
/// takes 5: `p_delay(1)` then `p_delay(2)`).
const CHAIN_STALL_TICKS: u64 = 12;
/// Ticks after the last item before the next op: `p_delay(2)` holds the
/// player 3 ticks (then the « not enough bars » box), and an op sent
/// while delayed is dropped.
const CHAIN_TAIL_TICKS: u32 = 4;
const STAND_WALK_TICKS: u64 = 800;

/// Anvil post of a run: the station at the brain's `--site` (any anvil,
/// Doric's included: the brain checked the quest), else the plain anvil
/// nearest (x, z) on that level (the Wilderness is open, user 2026-11-16).
pub fn anvil_post(site: Option<(i32, i32, i32)>, x: i32, z: i32, level: i32) -> Option<knowledge::stations::Station> {
    let anvils = knowledge::stations::stations_of("anvil");
    let distance = |post: &knowledge::stations::Station, from_x: i32, from_z: i32| (post.x - i64::from(from_x)).abs() + (post.z - i64::from(from_z)).abs();
    if let Some((site_x, site_z, site_level)) = site {
        return anvils.into_iter().filter(|post| post.level == i64::from(site_level)).min_by_key(|post| (distance(post, site_x, site_z), post.x, post.z));
    }
    anvils
        .into_iter()
        .filter(|post| post.level == i64::from(level) && knowledge::obtain::station_open(&post.loc, post.x, post.z))
        .min_by_key(|post| (distance(post, x, z), post.x, post.z))
}

/// Orthogonal stands of a 1x1 anvil, nearest the bot first (Doric's
/// anvil forces the north side: `smithing.loc` `forceapproach=north`).
fn anvil_stands(bot: &Bot, post: &knowledge::stations::Station) -> Vec<(i32, i32)> {
    let (x, z) = (post.x as i32, post.z as i32);
    if post.loc == "doric_anvil" {
        return vec![(x, z + 1)];
    }
    let mut stands = vec![(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)];
    stands.sort_by_key(|(stand_x, stand_z)| ((stand_x - bot.world.x).abs() + (stand_z - bot.world.z).abs(), *stand_x, *stand_z));
    stands
}

/// Server lines that end an anvil visit loud (quest gate, no hammer,
/// level), `None` for any other line.
pub fn anvil_refusal(line: &str) -> Option<ScriptError> {
    let lowered = line.to_lowercase();
    if lowered.contains("complete doric's quest") {
        return Some(crate::kit::cycle::blocked_quest("doric"));
    }
    if lowered.contains("need a hammer") {
        return Some(ScriptError::Blocked { missing: vec![HAMMER.to_string()] });
    }
    if lowered.contains("smithing level") {
        return Some(ScriptError::Failed(format!("server refused: {line}")));
    }
    None
}

/// True while the smithing columns (a main modal) are open.
fn columns_open(bot: &Bot) -> bool {
    bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "main")
}

/// Bar on the post anvil from the nearest stand that reaches ("can't
/// reach" moves on). True once the columns are open.
fn open_columns(bot: &mut Bot, post: &knowledge::stations::Station, bar: u16) -> Result<bool, ScriptError> {
    let (x, z, loc) = (post.x as i32, post.z as i32, post.loc_id as u16);
    for (stand_x, stand_z) in anvil_stands(bot, post) {
        match crate::skill_walk::walk_checked(bot, stand_x, stand_z, STAND_WALK_TICKS) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) if is_walk_abort(&error) => return Err(error),
            Err(_) => continue,
        }
        let seen = bot.world.messages.len();
        let (use_com, use_slot) = crate::quests::common::inv_slot(bot, bar)?;
        bot.op_loc_u(x, z, loc, bar, u16::from(use_slot), use_com)?;
        let target = bot.world.ticks + OPEN_TICKS;
        let mut unreachable = false;
        while bot.world.ticks < target && !columns_open(bot) && !unreachable {
            bot.pump(1)?;
            for line in bot.world.messages.get(seen..).unwrap_or(&[]).to_vec() {
                if let Some(error) = anvil_refusal(&line) {
                    let _ = bot.close_modal();
                    return Err(error);
                }
                unreachable |= line.to_lowercase().contains("can't reach");
            }
        }
        if columns_open(bot) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One anvil visit: open the columns, press Make (`button` 1/2/3 = 1, 5,
/// 10 items, `produce::make_press`) on the product cell, then follow the
/// server's chain (5 ticks an item): it ends once `items` are made or the
/// bars no longer make one (the server's « not enough bars » box), plus
/// the item's `p_delay(2)` tail so the next op is never sent while the
/// player is delayed. A chain that stalls first (level-up) ends after
/// the stall window; the caller re-presses for what is left
/// (`produce::chain_cut`). Returns the units made; none is loud.
pub fn smith_batch(bot: &mut Bot, post: &knowledge::stations::Station, forge: &AnvilPress, cell: (u8, u8), button: u8, items: u32) -> Result<u32, ScriptError> {
    let column_com = (bot.tables.com_id)(&format!("smithing:column{}", cell.0)).ok_or_else(|| ScriptError::Usage(format!("unknown smithing column{} at {:?}", cell.0, bot.position())))?;
    if !open_columns(bot, post, forge.bar)? {
        return Err(ScriptError::Failed(format!("smithing interface never opened at {:?} (anvil {} at ({},{}))", bot.position(), post.loc, post.x, post.z)));
    }
    let had = pack_total(bot, forge.product);
    bot.inv_button(button, forge.product, cell.1, column_com)?;
    let chain = Chain { input: forge.bar, per_item: forge.bars_each, items, stall_ticks: CHAIN_STALL_TICKS, tail_ticks: CHAIN_TAIL_TICKS };
    follow_chain(bot, &chain, |_| false)?;
    // The chain ends on a mesbox when the bars run out mid-press; a level
    // up opens one too: close whatever stayed.
    if bot.world.modal.is_some() {
        bot.close_modal()?;
        bot.pump(1)?;
    }
    let made = pack_total(bot, forge.product).saturating_sub(had);
    if made == 0 {
        let last = bot.world.messages.last().cloned().unwrap_or_default();
        return Err(ScriptError::Failed(format!("nothing smithed at {:?} (last line {last:?})", bot.position())));
    }
    Ok(made)
}

/// Bar, bars per item and product of one smithing press.
pub struct AnvilPress {
    pub bar: u16,
    pub bars_each: u32,
    pub product: u16,
}
