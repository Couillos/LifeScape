//! Craft recipes (smith, smelt, cook, leather, gem, fletch, brew).
//! Mirrors `gamedata/recipes.py`. Levels and XP are raw server values.

use crate::{Json, num, opt_flag, opt_str, str_int_map, table};

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

fn recipe_from_row(row: &Json) -> Recipe {
    let source = match row.get("extra") {
        Some(extra) => opt_str(extra, "row").unwrap_or_default(),
        None => String::new(),
    };
    Recipe {
        kind: opt_str(row, "kind").unwrap_or_default(),
        level: num(row, "level", 0),
        xp: num(row, "xp", 0),
        inputs: str_int_map(row, "inputs"),
        outputs: str_int_map(row, "outputs"),
        source,
        members: opt_flag(row, "members"),
    }
}

/// Every recipe, optionally filtered by kind.
pub fn all_recipes(kind: Option<&str>) -> Vec<Recipe> {
    table("recipes")
        .iter()
        .filter(|row| kind.is_none_or(|wanted| row.get("kind").and_then(Json::as_str) == Some(wanted)))
        .map(recipe_from_row)
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
        // Python: 58 cook recipes; steel_bar smelts from iron+coal;
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
