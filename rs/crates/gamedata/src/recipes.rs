//! Craft recipes (smith, smelt, cook, leather, gem, fletch, brew).
//! Levels and XP are raw server values.

/// One static recipe row (build-time data, mirrored into [`Recipe`] per call).
pub(crate) struct StaticRecipe {
    pub kind: &'static str,
    pub level: i64,
    pub xp: i64,
    pub inputs: &'static [(&'static str, i64)],
    pub outputs: &'static [(&'static str, i64)],
    pub source: &'static str,
    pub burnt: Option<&'static str>,
    pub members: Option<bool>,
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/recipes_generated.rs"));
}

/// One recipe row.
#[derive(Debug, Clone)]
pub struct Recipe {
    pub kind: String,
    pub level: i64,
    pub xp: i64,
    pub inputs: Vec<(String, i64)>,
    pub outputs: Vec<(String, i64)>,
    pub source: String,
    /// Burnt byproduct of this cooking row (`extra.burnt`), if any.
    pub burnt: Option<String>,
    pub members: Option<bool>,
}

fn pair_list(pairs: &[(&'static str, i64)]) -> Vec<(String, i64)> {
    pairs.iter().map(|(name, value)| ((*name).to_string(), *value)).collect()
}

fn recipe_from_static(row: &StaticRecipe) -> Recipe {
    Recipe {
        kind: row.kind.to_string(),
        level: row.level,
        xp: row.xp,
        inputs: pair_list(row.inputs),
        outputs: pair_list(row.outputs),
        source: row.source.to_string(),
        burnt: row.burnt.map(str::to_string),
        members: row.members,
    }
}

/// Every recipe, optionally filtered by kind.
pub fn all_recipes(kind: Option<&str>) -> Vec<Recipe> {
    generated::ALL
        .iter()
        .filter(|row| kind.is_none_or(|wanted| row.kind == wanted))
        .map(recipe_from_static)
        .collect()
}

/// Recipes producing `product` (case-insensitive on output names).
pub fn making(product: &str, kind: Option<&str>) -> Vec<Recipe> {
    let wanted = product.to_lowercase();
    all_recipes(kind)
        .into_iter()
        .filter(|entry| entry.outputs.iter().any(|(name, _)| name.to_lowercase() == wanted))
        .collect()
}

/// Recipes consuming `obj_name` (case-insensitive on input names).
pub fn using(obj_name: &str, kind: Option<&str>) -> Vec<Recipe> {
    let wanted = obj_name.to_lowercase();
    all_recipes(kind)
        .into_iter()
        .filter(|entry| entry.inputs.iter().any(|(name, _)| name.to_lowercase() == wanted))
        .collect()
}

/// Odds that one action of a recipe row (`Recipe::source`) yields its
/// product. The dump carries none, so the rows that can fail are read
/// from the server scripts: smelting iron fails half the time on F2P
/// (`skill_smithing/scripts/smelting/smelting.rs2`, `[queue,smelting_ore]`:
/// `randominc(1) = 1` → "The ore is too impure", the ore is consumed, no
/// bar, no XP; only the members ring of forging saves it).
pub fn success_odds(source: &str) -> f64 {
    match source {
        "smelting_iron_bar" => 0.5,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{all_recipes, making, success_odds, using};

    #[test]
    fn recipe_vectors() {
        // 58 cook recipes; steel_bar smelts from iron+coal;
        // iron_ore feeds 2 recipes.
        assert_eq!(all_recipes(Some("cook")).len(), 58);
        let steel = making("steel_bar", None);
        assert_eq!(steel.len(), 1);
        assert_eq!(steel[0].kind, "smelt");
        assert_eq!(steel[0].level, 30);
        assert_eq!(steel[0].xp, 175);
        assert!(steel[0].inputs.contains(&("iron_ore".to_string(), 1)));
        assert!(steel[0].inputs.contains(&("coal".to_string(), 2)));
        assert_eq!(steel[0].source, "smelting_steel_bar");
        assert_eq!(steel[0].members, Some(false));
        assert_eq!(using("iron_ore", None).len(), 2);
        assert!(making("STEEL_BAR", None).len() == 1);
    }

    /// Iron is the only smelt row that can fail (50 %), and the server
    /// script still says so: the iron branch rolls `randominc(1)` before
    /// the bar is added, every other bar is certain.
    #[test]
    fn smelt_success_odds_follow_the_server_script() {
        assert_eq!(success_odds("smelting_iron_bar"), 0.5);
        for bar in ["bronze_bar", "silver_bar", "steel_bar", "gold_bar", "mithril_bar", "adamantite_bar", "runite_bar"] {
            let row = making(bar, Some("smelt")).into_iter().find(|entry| entry.members == Some(false)).expect("F2P smelt row");
            assert_eq!(success_odds(&row.source), 1.0, "{bar} always yields");
        }
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script = manifest
            .ancestors()
            .map(|dir| dir.join("lostcity/content/scripts/skill_smithing/scripts/smelting/smelting.rs2"))
            .find(|candidate| candidate.is_file())
            .expect("../lostcity checkout next to LifeScape");
        let text = std::fs::read_to_string(&script).expect("smelting.rs2 readable");
        let queue = text.split("[queue,smelting_ore]").nth(1).expect("smelting_ore queue");
        let before_bar = queue.split("inv_add(inv, $product, 1)").next().expect("bar added");
        assert!(before_bar.contains("if ($product = iron_bar)") && before_bar.contains("randominc(1) = 1"), "iron 50 % roll moved in the server script");
        assert_eq!(before_bar.matches("randominc").count(), 1, "a new failing bar appeared in the server script");
    }
}
