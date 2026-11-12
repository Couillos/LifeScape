#![forbid(unsafe_code)]
//! Natural account names for `create-user` (user rule: no more
//! `b3gybcev`). A name reads like a player's: `olivia_king`, `tom_baker`,
//! `mira_stone`, sometimes a first name with a year-like number
//! (`ethan_92`). RS2 rule: 12 characters max, `a-z0-9_` (the underscore
//! shows as a space). Iron accounts keep their `Iron` prefix
//! (`IronOlivia`). Deterministic on the seed; the caller retries other
//! seeds when the name is taken.

const FIRST_NAMES: &[&str] = &[
    "adam", "alice", "amber", "anna", "arthur", "ben", "bella", "carl", "chloe", "clara", "dan", "daisy", "david", "eli",
    "ella", "emma", "eric", "erin", "ethan", "eva", "felix", "finn", "fiona", "frank", "george", "grace", "hana", "harry",
    "hugo", "ida", "isaac", "ivy", "jack", "jade", "james", "jane", "jim", "joe", "jonas", "julia", "kai", "kate", "kevin",
    "lara", "leo", "lily", "liam", "lucas", "lucy", "luke", "mark", "maya", "mia", "mila", "mira", "nathan", "nina", "noah",
    "nora", "oliver", "olivia", "oscar", "owen", "paul", "pia", "quinn", "rae", "ray", "rita", "rose", "ruby", "ryan", "sam",
    "sara", "sean", "sofia", "tara", "theo", "tim", "tom", "una", "vera", "victor", "will", "zack", "zoe",
];

/// Surnames, many from crafts and places (it is a medieval world).
const LAST_NAMES: &[&str] = &[
    "baker", "black", "brook", "brown", "carter", "clay", "cole", "cook", "cooper", "cross", "dale", "drake", "dunn", "ellis",
    "field", "fisher", "flint", "ford", "fox", "frost", "gray", "green", "hale", "hart", "hill", "holt", "hunt", "king",
    "knight", "lake", "lane", "mason", "miller", "moore", "north", "oak", "page", "park", "piper", "quill", "reed", "rhodes",
    "rivers", "rook", "rowe", "sage", "shaw", "smith", "snow", "stone", "storm", "swift", "tanner", "taylor", "thorn",
    "vale", "wade", "ward", "webb", "west", "wolfe", "wood", "wren", "young",
];

/// RS2 name budget.
pub const NAME_MAX: usize = 12;

fn mix(seed: u64, salt: u64) -> u64 {
    let mut word = seed.wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1);
    word ^= word << 13;
    word ^= word >> 7;
    word ^= word << 17;
    word
}

fn pick<'a>(list: &[&'a str], seed: u64, salt: u64) -> &'a str {
    list[(mix(seed, salt) % list.len() as u64) as usize]
}

/// One natural name for `seed` (never longer than 12).
pub fn natural_name(iron: bool, seed: u64) -> String {
    let first = pick(FIRST_NAMES, seed, 1);
    if iron {
        // `Iron` + a capitalised first name: `IronOlivia`.
        let mut name = String::from("Iron");
        let mut chars = first.chars();
        if let Some(head) = chars.next() {
            name.push(head.to_ascii_uppercase());
            name.extend(chars);
        }
        name.truncate(NAME_MAX);
        return name;
    }
    let last = pick(LAST_NAMES, seed, 2);
    let style = mix(seed, 3) % 10;
    let candidate = match style {
        // Mostly first_last, like the hand-made `olivia_king`.
        0..=6 => format!("{first}_{last}"),
        // Sometimes glued, sometimes a number a player would pick.
        7 => format!("{first}{last}"),
        8 => format!("{first}_{}", 70 + mix(seed, 4) % 30),
        _ => format!("{last}_{first}"),
    };
    if candidate.len() <= NAME_MAX {
        candidate
    } else {
        // Too long: keep the first name and the surname's initial.
        let initial = last.chars().next().unwrap_or('x');
        let short = format!("{first}_{initial}");
        short.chars().take(NAME_MAX).collect()
    }
}

/// True for a name RS2 accepts (length, charset, no edge underscore).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.chars().all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
        && !name.starts_with('_')
        && !name.ends_with('_')
}

#[cfg(test)]
mod tests {
    use super::{natural_name, valid_name, NAME_MAX};

    #[test]
    fn names_are_natural_valid_and_varied() {
        let names: Vec<String> = (1..400u64).map(|seed| natural_name(false, seed)).collect();
        assert!(names.iter().all(|name| valid_name(name)), "all valid: {:?}", names.iter().filter(|name| !valid_name(name)).collect::<Vec<_>>());
        assert!(names.iter().all(|name| name.len() <= NAME_MAX));
        assert!(names.iter().filter(|name| name.contains('_')).count() > 250, "mostly first_last");
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert!(unique.len() > 350, "varied: {} distinct of {}", unique.len(), names.len());
        assert_eq!(natural_name(false, 7), natural_name(false, 7), "seeded");
        let iron = natural_name(true, 7);
        assert!(iron.starts_with("Iron") && valid_name(&iron), "{iron}");
    }
}
