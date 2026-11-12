use super::*;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// Server default look (`Player.ts` `body` / `colors`, gender 0).
const SERVER_DEFAULT: PlayerDesign = PlayerDesign {
    gender: GENDER_MAN,
    kits: [Some(0), Some(10), Some(18), Some(26), Some(33), Some(36), Some(42)],
    colours: [0, 0, 0, 0, 0],
};

#[test]
fn server_default_is_valid_and_rejections_mirror_the_handler() {
    assert!(is_valid(&SERVER_DEFAULT));
    let mut bad_gender = SERVER_DEFAULT;
    bad_gender.gender = 2;
    assert!(!is_valid(&bad_gender), "gender > 1 refused");
    let mut demon_hair = SERVER_DEFAULT;
    demon_hair.kits[0] = Some(9);
    assert!(!is_valid(&demon_hair), "disabled kit refused");
    let mut woman_kit_on_man = SERVER_DEFAULT;
    woman_kit_on_man.kits[0] = Some(45);
    assert!(!is_valid(&woman_kit_on_man), "kit type must match gender + part");
    let mut missing_man_jaw = SERVER_DEFAULT;
    missing_man_jaw.kits[1] = None;
    assert!(!is_valid(&missing_man_jaw), "only the woman jaw may be -1");
    let mut hair_colour_out = SERVER_DEFAULT;
    hair_colour_out.colours[0] = 12;
    assert!(!is_valid(&hair_colour_out), "colour index past its row refused");
    let woman = PlayerDesign {
        gender: GENDER_WOMAN,
        kits: [Some(45), None, Some(56), Some(61), Some(67), Some(70), Some(79)],
        colours: [11, 15, 15, 5, 7],
    };
    assert!(is_valid(&woman));
    let mut woman_with_jaw = woman;
    woman_with_jaw.kits[1] = Some(10);
    assert!(!is_valid(&woman_with_jaw), "no woman_jaw kit exists");
}

#[test]
fn generated_designs_stay_within_the_server_sets() {
    for seed in 0..20_000u64 {
        let design = design_for_seed(seed);
        assert!(is_valid(&design), "seed {seed}: {design:?}");
        assert!(design.gender <= 1);
        let allowed = kits_for_gender(design.gender).expect("gender");
        for (slot, kit) in design.kits.iter().enumerate() {
            match kit {
                Some(id) => assert!(allowed[slot].contains(id), "seed {seed} slot {slot}"),
                None => assert!(design.gender == GENDER_WOMAN && slot == 1, "seed {seed}: only the woman jaw is empty"),
            }
        }
        for (slot, colour) in design.colours.iter().enumerate() {
            assert!(*colour < COLOUR_COUNTS[slot], "seed {seed} colour {slot}");
        }
    }
}

#[test]
fn every_choice_is_reachable() {
    let mut genders = HashSet::new();
    let mut kits_seen = HashSet::new();
    let mut colours_seen = HashSet::new();
    for seed in 0..20_000u64 {
        let design = design_for_seed(seed);
        genders.insert(design.gender);
        for (slot, kit) in design.kits.iter().enumerate() {
            kits_seen.insert((design.gender, slot, *kit));
        }
        for (slot, colour) in design.colours.iter().enumerate() {
            colours_seen.insert((slot, *colour));
        }
    }
    assert_eq!(genders.len(), 2, "both genders drawn");
    for gender in [GENDER_MAN, GENDER_WOMAN] {
        let allowed = kits_for_gender(gender).expect("gender");
        for (slot, choices) in allowed.iter().enumerate() {
            for kit in choices.iter() {
                assert!(kits_seen.contains(&(gender, slot, Some(*kit))), "gender {gender} slot {slot} kit {kit} never drawn");
            }
        }
    }
    for (slot, count) in COLOUR_COUNTS.iter().enumerate() {
        for colour in 0..*count {
            assert!(colours_seen.contains(&(slot, colour)), "colour {slot}:{colour} never drawn");
        }
    }
}

#[test]
fn same_account_same_look_and_accounts_differ() {
    assert_eq!(design_for_account("t01"), design_for_account("t01"), "re-run keeps the look");
    assert_eq!(design_for_account("T01"), design_for_account(" t01 "), "names are case-insensitive");
    assert_ne!(design_for_account("t01"), SERVER_DEFAULT, "the default man is not reused");
    let fleet: Vec<String> = (1..=13).map(|index| format!("t{index:02}")).collect();
    let fleet_designs: HashSet<PlayerDesign> = fleet.iter().map(|name| design_for_account(name)).collect();
    assert_eq!(fleet_designs.len(), fleet.len(), "13 bots, 13 looks");
    let crowd: HashSet<PlayerDesign> = (0..1000).map(|index| design_for_account(&format!("bot{index}"))).collect();
    assert!(crowd.len() >= 995, "1000 accounts only gave {} looks", crowd.len());
}

// -- cross-check against the server content -------------------------------

fn lostcity_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .map(|dir| dir.join("lostcity"))
        .find(|candidate| candidate.join("content/pack/idk.pack").is_file())
        .expect("../lostcity checkout (content/pack/idk.pack) next to LifeScape")
}

/// Bodypart index of an `.idk` `type=` value (`tools/pack/config/IdkConfig.ts`).
fn bodypart_index(type_name: &str) -> usize {
    let parts = [
        "man_hair", "man_jaw", "man_torso", "man_arms", "man_hands", "man_legs", "man_feet",
        "woman_hair", "woman_jaw", "woman_torso", "woman_arms", "woman_hands", "woman_legs", "woman_feet",
    ];
    parts.iter().position(|name| *name == type_name).unwrap_or_else(|| panic!("unknown idk type {type_name}"))
}

#[test]
fn tables_match_the_server_content() {
    let root = lostcity_root();
    let pack = std::fs::read_to_string(root.join("content/pack/idk.pack")).expect("idk.pack");
    let mut ids: BTreeMap<String, u8> = BTreeMap::new();
    for line in pack.lines().filter(|line| !line.trim().is_empty()) {
        let (id, name) = line.split_once('=').expect("id=name");
        ids.insert(name.trim().to_string(), id.trim().parse().expect("id"));
    }
    // (bodypart index, kit id) of every enabled kit.
    let mut expected: [Vec<u8>; 14] = Default::default();
    for file in ["man.idk", "woman.idk"] {
        let text = std::fs::read_to_string(root.join("content/scripts/player/configs").join(file)).expect("idk config");
        let mut sections: Vec<(String, Option<usize>, bool)> = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
                sections.push((name.to_string(), None, false));
            } else if let Some(section) = sections.last_mut() {
                if let Some(type_name) = line.strip_prefix("type=") {
                    section.1 = Some(bodypart_index(type_name));
                } else if line == "disable=yes" {
                    section.2 = true;
                }
            }
        }
        for (name, part, disabled) in sections {
            let id = *ids.get(&name).unwrap_or_else(|| panic!("{name} missing from idk.pack"));
            let part = part.unwrap_or_else(|| panic!("{name} has no type"));
            if !disabled {
                expected[part].push(id);
            }
        }
    }
    for list in expected.iter_mut() {
        list.sort_unstable();
    }
    for slot in 0..7 {
        assert_eq!(MAN_KITS[slot], expected[slot].as_slice(), "man {}", BODY_PARTS[slot]);
        assert_eq!(WOMAN_KITS[slot], expected[slot + 7].as_slice(), "woman {}", BODY_PARTS[slot]);
    }

    // Colour rows: `static readonly DESIGN_BODY_COLORS: number[][] = [ [..], .. ];`
    let player = std::fs::read_to_string(root.join("engine/src/engine/entity/Player.ts")).expect("Player.ts");
    let start = player.find("DESIGN_BODY_COLORS").expect("DESIGN_BODY_COLORS");
    let block = &player[start..];
    let block = &block[block.find("= [").expect("table start") + 3..block.find("];").expect("table end")];
    let rows: Vec<usize> = block
        .split(']')
        .filter_map(|row| row.split_once('[').map(|(_, values)| values.split(',').filter(|value| !value.trim().is_empty()).count()))
        .collect();
    let counts: Vec<usize> = COLOUR_COUNTS.iter().map(|count| usize::from(*count)).collect();
    assert_eq!(rows, counts, "colour row lengths");
}
