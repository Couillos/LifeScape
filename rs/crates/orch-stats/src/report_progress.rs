#![forbid(unsafe_code)]
//! Détecteur de boucles muettes côté orchestrateur (pur, par rapport).
//!
//! Un rapport de tâche dit déjà tout ce qu'une fenêtre a produit : XP par
//! compétence, flux d'objets, ticks, ouvertures de banque. Deux motifs
//! sont des boucles muettes, jamais un rythme normal :
//! - `no_output` : une fenêtre finie sur son budget (≥ `MIN_TICKS`) sans
//!   un point d'XP ni un objet gagné (live 2026-11-14 : harry_page,
//!   will_field, sara_miller, `pottery piedish_unfired`, ~1400 banques en
//!   30 min, « ok load » à chaque tour, aucun plat) ;
//! - `bank_churn` : au moins `CHURN_MIN_OPENS` ouvertures de banque et
//!   moins d'unités gagnées que d'ouvertures (une banque par objet ou
//!   pire : dépôt/retrait en boucle).
//! Le marché (fenêtres d'attente de remplissage), les trajets (`walkto`),
//! le tutoriel et l'équipement ne produisent rien par nature : exclus.
//! L'appelant écrit la ligne `error_events` (`no_progress`) et le cerveau
//! compte l'échec du nœud (`brain_fold`).

use lifescape_json::Value;

/// Fenêtre minimale jugée : une fenêtre plus courte n'a pas eu le temps
/// d'une charge (marche jusqu'au site comprise).
pub const MIN_TICKS: i64 = 1_000;
/// Ouvertures de banque à partir desquelles le ratio est jugé (une
/// tournée normale banque toutes les ~150–400 ticks : 15–40 par fenêtre).
pub const CHURN_MIN_OPENS: i64 = 40;
/// Type de ligne `error_events` et préfixe des messages.
pub const KIND: &str = "no_progress";

/// Ce que le détecteur a vu dans un rapport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `no_output` | `bank_churn`.
    pub pattern: &'static str,
    /// `no progress: …` (lisible, porté au tableau de bord et au nœud).
    pub message: String,
}

/// Activités qui ne produisent rien par nature (attente, trajet, setup).
fn exempt(activity: &str) -> bool {
    activity.starts_with("market") || matches!(activity, "walkto" | "tutorial" | "equip" | "")
}

/// XP gagnée par le rapport, toutes compétences.
pub fn xp_gained(report: &Value) -> i64 {
    report.get("xp").as_arr().map(|rows| rows.iter().filter_map(|row| row.get("xp").as_i64()).filter(|gain| *gain > 0).sum()).unwrap_or(0)
}

/// Unités d'objets gagnées (flux positifs, pièces comprises).
pub fn units_gained(report: &Value) -> i64 {
    report.get("items").as_arr().map(|rows| rows.iter().filter_map(|row| row.get("qty").as_i64()).filter(|qty| *qty > 0).sum()).unwrap_or(0)
}

/// Juge un rapport poussé : `Some` quand la fenêtre est une boucle muette.
/// Un arrêt opérateur (`stopped`) n'est jamais jugé ; un échec typé ou un
/// blocage porte déjà sa propre erreur, seul le motif banque y est ajouté.
pub fn assess(report: &Value) -> Option<Finding> {
    let activity = report.get("activity").as_str().unwrap_or("");
    let stop = report.get("stop").as_str().unwrap_or("");
    if exempt(activity) || stop == "stopped" {
        return None;
    }
    let ticks = report.get("ticks").as_i64().unwrap_or(0);
    let opens = report.get("bank_opens").as_i64().unwrap_or(0);
    let xp = xp_gained(report);
    let units = units_gained(report);
    if stop == "budget_exhausted" && ticks >= MIN_TICKS && xp == 0 && units == 0 {
        return Some(Finding {
            pattern: "no_output",
            message: format!("no progress: {activity} ran {ticks} ticks to its budget with 0 items, 0 XP ({opens} bank opens)"),
        });
    }
    if opens >= CHURN_MIN_OPENS && units < opens && xp == 0 {
        return Some(Finding {
            pattern: "bank_churn",
            message: format!("no progress: {activity} opened the bank {opens} times in {ticks} ticks for {units} items and 0 XP"),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::assess;

    fn report(text: &str) -> lifescape_json::Value {
        lifescape_json::parse(text).expect("json")
    }

    /// Live harry_page n133 (2026-11-14) : 6076 ticks au budget, aucun
    /// flux, aucune XP — la fenêtre est une boucle muette.
    #[test]
    fn a_budget_window_without_output_is_no_progress() {
        let silent = report(r#"{"activity":"pottery","stop":"budget_exhausted","ticks":6076,"items":[],"xp":[],"bank_opens":280}"#);
        let found = assess(&silent).expect("silent loop");
        assert_eq!(found.pattern, "no_output");
        assert!(found.message.starts_with("no progress: pottery ran 6076 ticks"), "{}", found.message);
        let short = report(r#"{"activity":"pottery","stop":"budget_exhausted","ticks":400,"items":[],"xp":[]}"#);
        assert_eq!(assess(&short), None, "too short to judge");
        let working = report(r#"{"activity":"pottery","stop":"budget_exhausted","ticks":6000,"items":[{"name":"Pie dish","qty":20}],"xp":[{"skill":"crafting","xp":3000}],"bank_opens":12}"#);
        assert_eq!(assess(&working), None, "a producing window is fine");
        let xp_only = report(r#"{"activity":"firemaking","stop":"budget_exhausted","ticks":6000,"items":[{"name":"Logs","qty":-80}],"xp":[{"skill":"firemaking","xp":3200}]}"#);
        assert_eq!(assess(&xp_only), None, "XP is progress");
    }

    /// Le marché attend ses remplissages, un trajet ne produit rien, un
    /// arrêt opérateur n'est pas une fenêtre : jamais jugés.
    #[test]
    fn waiting_and_operator_stops_are_exempt() {
        for activity in ["market", "walkto", "tutorial", "equip"] {
            let text = format!(r#"{{"activity":"{activity}","stop":"budget_exhausted","ticks":6000,"items":[],"xp":[]}}"#);
            assert_eq!(assess(&report(&text)), None, "{activity}");
        }
        let stopped = report(r#"{"activity":"pottery","stop":"stopped","ticks":6000,"items":[],"xp":[],"bank_opens":300}"#);
        assert_eq!(assess(&stopped), None);
    }

    /// Une banque par objet (ou pire) sans XP : barattage, même quand la
    /// fenêtre gagne quelques unités ; une tournée de 27 gemmes taillées
    /// par banque n'en est pas une.
    #[test]
    fn bank_churn_is_flagged_by_ratio() {
        let churn = report(r#"{"activity":"fill_water","stop":"budget_exhausted","ticks":6000,"items":[{"name":"Jug of water","qty":12}],"xp":[],"bank_opens":90}"#);
        assert_eq!(assess(&churn).map(|found| found.pattern), Some("bank_churn"));
        let healthy = report(r#"{"activity":"fill_water","stop":"budget_exhausted","ticks":6000,"items":[{"name":"Jug of water","qty":1200}],"xp":[],"bank_opens":45}"#);
        assert_eq!(assess(&healthy), None);
        let failed = report(r#"{"activity":"pottery","stop":"failed","ticks":200,"items":[],"xp":[],"bank_opens":3}"#);
        assert_eq!(assess(&failed), None, "a typed failure carries its own row");
    }
}
