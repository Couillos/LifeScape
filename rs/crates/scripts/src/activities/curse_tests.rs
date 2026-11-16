use super::{choose_step, curse_refusal, debuff_level, run_curse, CurseFoes, CurseRefusal, CurseStep, LOWERED_BAN_TICKS, MAGIC_STAT};

/// The server's lines (`player_magic.rs2`, `player_combat.rs2`) by what
/// the loop does about them.
#[test]
fn server_refusals_are_classified() {
    assert_eq!(curse_refusal("Your foe's attack has already been lowered."), Some(CurseRefusal::Lowered));
    assert_eq!(curse_refusal("Your foe's defence has already been weakened."), Some(CurseRefusal::Lowered));
    assert_eq!(curse_refusal("Your target is already held by a magical force."), Some(CurseRefusal::Lowered));
    assert_eq!(curse_refusal("Your target is currently immune to that spell."), Some(CurseRefusal::Lowered));
    assert_eq!(curse_refusal("I'm already under attack!"), Some(CurseRefusal::UnderAttack));
    assert_eq!(curse_refusal("Someone else is fighting that."), Some(CurseRefusal::Taken));
    assert_eq!(curse_refusal("I can't reach that!"), Some(CurseRefusal::Taken));
    assert_eq!(curse_refusal("You do not have enough Body Runes to cast this spell."), None);
}

/// The four F2P debuffs from the dump, nothing else.
#[test]
fn debuffs_come_from_the_dump() {
    assert_eq!(debuff_level("confuse"), Some(3));
    assert_eq!(debuff_level("weaken"), Some(11));
    assert_eq!(debuff_level("curse"), Some(19));
    assert_eq!(debuff_level("bind"), Some(20));
    assert_eq!(debuff_level("fire_strike"), None, "a strike deals damage");
    assert_eq!(debuff_level("vulnerability"), None, "members");
}

/// An aggressor is the only target: cast on it until a refusal says its
/// stat is down, then finish it; else the nearest foe not banned.
#[test]
fn the_step_follows_the_server_rules() {
    let mut foes = CurseFoes::default();
    let camp = [(7u16, 3230, 3298), (9, 3226, 3296), (11, 3290, 3290)];
    assert_eq!(choose_step(None, &camp, (3225, 3295), &foes, 100), CurseStep::Cast(9), "nearest");
    foes.ban(9, 150);
    assert_eq!(choose_step(None, &camp, (3225, 3295), &foes, 100), CurseStep::Cast(7), "a banned foe is skipped");
    assert_eq!(choose_step(None, &camp, (3225, 3295), &foes, 150), CurseStep::Cast(9), "the ban runs out");
    assert_eq!(choose_step(None, &camp[2..], (3225, 3295), &foes, 100), CurseStep::Search, "out of the radius");
    assert_eq!(choose_step(Some(7), &camp, (3225, 3295), &foes, 100), CurseStep::Cast(7), "a splashed aggressor takes the next cast");
    foes.mark_lowered(7, 100);
    assert!(foes.is_lowered(7, 100 + LOWERED_BAN_TICKS - 1) && !foes.is_lowered(7, 100 + LOWERED_BAN_TICKS));
    assert_eq!(choose_step(Some(7), &camp, (3225, 3295), &foes, 120), CurseStep::Finish(7), "a lowered aggressor is finished");
}

/// The brain's order (`mapping`: camp, `--spell`, level, site) parses:
/// the camp rides positionally like the fight scripts.
#[test]
fn the_brains_order_parses() {
    let args: Vec<String> = ["chickens", "--spell", "confuse", "--level", "magic", "10", "--site", "3233,3295,0", "--node", "n4"].iter().map(|arg| arg.to_string()).collect();
    let spec = crate::kit::cycle::parse_order("curse", &args).expect("curse parses");
    assert_eq!(spec.resource, vec!["chickens".to_string()]);
    assert_eq!(spec.spell.as_deref(), Some("confuse"));
    assert_eq!((spec.targets.level_skill.as_deref(), spec.targets.level), (Some("magic"), Some(10)));
    assert_eq!(spec.site, Some((3233, 3295, 0)));
}

/// Order checks before any walk: an unknown spell is a contract bug, a
/// low level blocks typed, an unfunded pack blocks on the exact runes.
#[test]
fn orders_fail_typed_offline() {
    let mut bot = crate::kit::spell_load::tests::stocked_bot(&[], &[]);
    bot.world.stats.insert(MAGIC_STAT, (3, 0));
    let args = |spell: &str| vec!["chickens".to_string(), "--spell".to_string(), spell.to_string()];
    assert!(matches!(run_curse(&mut bot, &args("fire_strike")), Err(crate::ScriptError::Usage(_))));
    assert!(matches!(run_curse(&mut bot, &["chickens".to_string()]), Err(crate::ScriptError::Usage(_))), "no spell");
    match run_curse(&mut bot, &args("weaken")) {
        Err(crate::ScriptError::Blocked { missing }) => assert!(missing.iter().any(|need| need.contains("magic")), "{missing:?}"),
        other => panic!("weaken at magic 3 blocks on the level, got {other:?}"),
    }
    match run_curse(&mut bot, &args("confuse")) {
        Err(crate::ScriptError::Blocked { mut missing }) => {
            missing.sort();
            assert_eq!(missing, vec!["bodyrune".to_string(), "earthrune".to_string(), "waterrune".to_string()]);
        }
        other => panic!("no runes anywhere blocks on them, got {other:?}"),
    }
}
