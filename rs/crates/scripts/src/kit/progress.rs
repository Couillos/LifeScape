//! Garde de progrès : aucune boucle de charge ne tourne en silence.
//!
//! `cycle::run` marque l'état après chaque pas (XP totale, détenu par
//! objet = sac + porté + dernière banque connue, ouvertures de banque,
//! position, tick). Un pas progresse quand l'XP totale monte ou qu'un
//! objet dépasse le plus haut détenu vu depuis le dernier progrès : un
//! dépôt ou un retrait déplace un objet sans changer son total, un
//! aller-retour banque qui rend ce qu'il a pris ne compte jamais (live
//! 2026-11-14 : `pottery piedish_unfired` retirait l'argile, la
//! redéposait et loggait « ok load » ~1400 fois en 30 min, sans eau).
//! Les combats gagnent de l'XP à chaque mise à mort : une mise à mort est
//! un progrès.
//!
//! Sans progrès, la garde lève `ScriptError::Failed("no progress: …")`
//! avec le motif vu :
//! - `IDLE_UNITS` charges finies (`UnitDone`) d'affilée sans rien ;
//! - `IDLE_STEPS` pas (reprises comprises) et `IDLE_STEP_TICKS` ticks ;
//! - `IDLE_BANK_OPENS` ouvertures de banque sur au moins deux pas
//!   (barattage dépôt/retrait) ;
//! - `IDLE_TICKS` ticks sans rien, quel que soit le nombre de pas.
//! Le message nomme les positions de fin de pas (aller-retour entre deux
//! points), les ouvertures de banque et la durée. Le cerveau compte
//! l'échec du nœud (`brain_fold`), le tableau des erreurs le range sous
//! `no_progress`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use crate::ScriptError;

/// Charges finies d'affilée sans progrès avant l'échec.
pub const IDLE_UNITS: u32 = 3;
/// Pas (reprises comprises) d'affilée sans progrès avant l'échec, à
/// condition d'avoir aussi duré `IDLE_STEP_TICKS` (re-pick rapides).
pub const IDLE_STEPS: u32 = 12;
pub const IDLE_STEP_TICKS: u64 = 1_500;
/// Ouvertures de banque sans progrès (sur au moins deux pas).
pub const IDLE_BANK_OPENS: u64 = 6;
/// Ticks sans progrès, quel que soit le rythme des pas.
pub const IDLE_TICKS: u64 = 3_000;

/// État observé à une fin de pas.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mark {
    pub xp_total: i64,
    /// Détenu par objet : sac + porté + dernière banque connue.
    pub held: HashMap<u16, u64>,
    pub bank_opens: u64,
    pub tick: u64,
    pub position: (i32, i32, i32),
}

/// Marque l'état courant du bot (lecture seule).
pub fn mark(bot: &Bot) -> Mark {
    let mut held: HashMap<u16, u64> = HashMap::new();
    for (_, (obj, count)) in bot.backpack().1 {
        *held.entry(obj).or_insert(0) += u64::from(count);
    }
    let mut add_com = |name: &str| {
        if let Some(slots) = (bot.tables.com_id)(name).and_then(|com| bot.world.inv_known(com)) {
            for (obj, count) in slots.values() {
                *held.entry(*obj).or_insert(0) += u64::from(*count);
            }
        }
    };
    add_com(crate::kit::gear::WORN_COM_NAME);
    add_com("bank_main:inv");
    Mark {
        xp_total: bot.world.stats.values().map(|(_, xp)| i64::from(*xp)).sum(),
        held,
        bank_opens: bot.bank_opens,
        tick: bot.world.ticks,
        position: bot.position(),
    }
}

/// Garde d'une boucle : plus hauts vus depuis le dernier progrès et
/// compteurs de la période muette en cours.
#[derive(Debug, Clone)]
pub struct ProgressGuard {
    activity: String,
    xp_high: i64,
    held_high: HashMap<u16, u64>,
    since_tick: u64,
    since_opens: u64,
    idle_units: u32,
    idle_steps: u32,
    idle_spots: Vec<(i32, i32, i32)>,
}

impl ProgressGuard {
    /// Garde armée sur l'état de départ.
    pub fn start(activity: &str, start: &Mark) -> ProgressGuard {
        ProgressGuard {
            activity: activity.to_string(),
            xp_high: start.xp_total,
            held_high: start.held.clone(),
            since_tick: start.tick,
            since_opens: start.bank_opens,
            idle_units: 0,
            idle_steps: 0,
            idle_spots: Vec::new(),
        }
    }

    /// Progrès depuis le dernier : XP au-dessus du plus haut, ou un objet
    /// au-dessus de son plus haut détenu.
    pub fn progressed(&self, now: &Mark) -> bool {
        now.xp_total > self.xp_high || now.held.iter().any(|(obj, count)| *count > self.held_high.get(obj).copied().unwrap_or(0))
    }

    /// Une pause marché a livré des objets : le détenu a baissé pour de
    /// bon (un échange, jamais un aller-retour de banque). Le plus haut vu
    /// redescend à l'état courant, sinon les prises suivantes ne le
    /// dépassent jamais (live 2026-11-22, ida_reed : 1 rune de l'esprit
    /// ramassée par pas, 4 800 en stock puis 4 400 livrés, « no progress:
    /// ground_pickup 26 steps in a row » sur des prises réelles). Les
    /// compteurs de la période muette ne bougent pas.
    pub fn rebase_after_delivery(&mut self, now: &Mark) {
        self.held_high = now.held.clone();
    }

    /// Note un pas fini (`unit` = charge `UnitDone`) ; `Err` typé quand la
    /// période muette dépasse un seuil.
    pub fn observe(&mut self, now: &Mark, unit: bool) -> Result<(), ScriptError> {
        if self.progressed(now) {
            self.xp_high = self.xp_high.max(now.xp_total);
            for (obj, count) in &now.held {
                let high = self.held_high.entry(*obj).or_insert(0);
                *high = (*high).max(*count);
            }
            self.since_tick = now.tick;
            self.since_opens = now.bank_opens;
            self.idle_units = 0;
            self.idle_steps = 0;
            self.idle_spots.clear();
            return Ok(());
        }
        self.idle_steps += 1;
        if unit {
            self.idle_units += 1;
        }
        if !self.idle_spots.contains(&now.position) {
            self.idle_spots.push(now.position);
        }
        let ticks = now.tick.saturating_sub(self.since_tick);
        let opens = now.bank_opens.saturating_sub(self.since_opens);
        let pattern = if self.idle_units >= IDLE_UNITS {
            Some(format!("{} loads in a row produced nothing", self.idle_units))
        } else if opens >= IDLE_BANK_OPENS && self.idle_steps >= 2 {
            Some(format!("bank churn ({opens} bank opens, holdings unchanged)"))
        } else if self.idle_steps >= IDLE_STEPS && ticks >= IDLE_STEP_TICKS {
            Some(format!("{} steps in a row produced nothing", self.idle_steps))
        } else if ticks >= IDLE_TICKS {
            Some(format!("{ticks} ticks without output"))
        } else {
            None
        };
        match pattern {
            Some(pattern) => Err(ScriptError::Failed(format!(
                "no progress: {} {pattern} (0 product, 0 XP, {opens} bank opens, {ticks} ticks, {}) at {:?}",
                self.activity,
                self.travel(),
                now.position
            ))),
            None => Ok(()),
        }
    }

    /// Positions de fin de pas de la période muette : un aller-retour
    /// entre deux points se lit tel quel.
    fn travel(&self) -> String {
        let spots: Vec<String> = self.idle_spots.iter().map(|(level, x, z)| format!("({x},{z},{level})")).collect();
        match spots.len() {
            0 => "no move".to_string(),
            1 => format!("stuck at {}", spots[0]),
            2 => format!("back and forth {} <-> {}", spots[0], spots[1]),
            _ => format!("{} spots: {}", spots.len(), spots.join(" ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mark, ProgressGuard, IDLE_TICKS};

    /// Live 2026-11-22 (ida_reed, ground:mindrune): a market pause delivered
    /// 400 of 4 800 runes; the one rune picked per step never beat the old
    /// high. Rebased after the delivery, a pick is progress again.
    #[test]
    fn a_pick_after_a_delivery_is_progress() {
        let site = (0, 3206, 3208);
        let mut guard = ProgressGuard::start("ground_pickup", &at(0, &[(558, 4800)], 0, 100, site));
        let after = at(0, &[(558, 4401)], 3, 900, site);
        assert!(!guard.progressed(&after), "below the old high: no progress without a rebase");
        guard.rebase_after_delivery(&after);
        assert!(guard.progressed(&at(0, &[(558, 4402)], 3, 1000, site)), "one rune picked after the delivery");
        assert!(!guard.progressed(&at(0, &[(558, 4401)], 3, 1000, site)), "a round trip still counts for nothing");
    }

    fn at(xp_total: i64, held: &[(u16, u64)], bank_opens: u64, tick: u64, position: (i32, i32, i32)) -> Mark {
        Mark { xp_total, held: held.iter().copied().collect(), bank_opens, tick, position }
    }

    /// Live 2026-11-14 (harry_page, `pottery piedish_unfired`) : chaque
    /// charge retirait 14 argiles, ne mélangeait rien (pas d'eau) et les
    /// redéposait — l'argile ne fait que changer de côté, 3 ouvertures de
    /// banque par charge. La garde échoue au barattage, avant la 3e charge.
    #[test]
    fn a_withdraw_then_deposit_loop_fails_loud() {
        let bank = (0, 3185, 3436);
        let start = at(1000, &[(434, 60)], 0, 800, bank);
        let mut guard = ProgressGuard::start("pottery", &start);
        assert!(guard.observe(&at(1000, &[(434, 60)], 3, 865, bank), true).is_ok(), "one idle load");
        let error = guard.observe(&at(1000, &[(434, 60)], 6, 930, bank), true).expect_err("bank churn");
        let text = error.to_string();
        assert!(text.contains("no progress: pottery bank churn (6 bank opens"), "{text}");
        assert!(text.contains("stuck at (3185,3436,0)"), "{text}");
    }

    /// Une charge qui fabrique (XP ou objet au-dessus du plus haut) remet
    /// tout à zéro ; un objet qui revient à son ancien plus haut n'est pas
    /// un progrès (le retrait de ce qu'on vient de déposer).
    #[test]
    fn production_resets_and_oscillation_does_not() {
        let site = (0, 3087, 3409);
        let start = at(0, &[(1, 10), (2, 0)], 0, 0, site);
        let mut guard = ProgressGuard::start("pottery", &start);
        assert!(guard.observe(&at(0, &[(1, 4), (2, 6)], 1, 100, site), true).is_ok(), "6 dishes made");
        assert!(guard.observe(&at(0, &[(1, 4), (2, 6)], 2, 200, site), true).is_ok());
        assert!(guard.observe(&at(500, &[(1, 4), (2, 6)], 3, 300, site), true).is_ok(), "XP alone is progress");
        let mut churn = ProgressGuard::start("fill_water", &at(0, &[(7, 5)], 0, 0, site));
        for (count, tick) in [(0u64, 50u64), (5, 100)] {
            assert!(churn.observe(&at(0, &[(7, count)], 0, tick, site), false).is_ok());
        }
        assert!(churn.observe(&at(0, &[(7, 5)], 0, 150, (0, 3090, 3410)), true).is_ok(), "one idle load");
        assert!(churn.observe(&at(0, &[(7, 0)], 0, 200, site), true).is_ok(), "two idle loads");
        let error = churn.observe(&at(0, &[(7, 5)], 0, 250, site), true).expect_err("third idle load");
        assert!(error.to_string().contains("3 loads in a row produced nothing"), "{error}");
        assert!(error.to_string().contains("back and forth (3087,3409,0) <-> (3090,3410,0)"), "{error}");
    }

    /// Des reprises rapides (re-pick, vache partie) ne suffisent pas : il
    /// faut aussi la durée ; une période muette trop longue échoue même
    /// en un seul pas.
    #[test]
    fn quick_retries_need_time_and_long_silence_fails() {
        let spot = (0, 3255, 3270);
        let mut guard = ProgressGuard::start("milk", &at(0, &[], 0, 0, spot));
        for step in 1..=20u64 {
            assert!(guard.observe(&at(0, &[], 0, step * 10, spot), false).is_ok(), "step {step}: quick re-picks");
        }
        let mut slow = ProgressGuard::start("woodcut", &at(0, &[], 0, 0, spot));
        let error = slow.observe(&at(0, &[], 0, IDLE_TICKS, spot), false).expect_err("silent too long");
        assert!(error.to_string().contains("ticks without output"), "{error}");
    }
}
