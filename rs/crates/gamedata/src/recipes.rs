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

#[cfg(test)]
mod tests {
    use super::{all_recipes, making, using};

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
}
