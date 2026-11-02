import unittest


from gamedata import quest_requirements
from lifescape.quests import PlayerState, check
from gamedata.quest_requirements import QuestDependency, QuestRequirements


def state(**overrides):
    values = {
        "levels": {},
        "inventory": {},
        "worn": {},
        "bank": {},
        "varps": {},
        "quest_points": 0,
    }
    values.update(overrides)
    return PlayerState(**values)


class QuestTableTest(unittest.TestCase):
    def test_eighteen_f2p_quests(self):
        self.assertEqual(len(quest_requirements.all_f2p_quests()), 18)

    def test_every_item_and_varp_resolves(self):
        for quest in quest_requirements.all_f2p_quests():
            self.assertTrue(quest.completion_varps, quest.key)
            self.assertFalse(quest.members, quest.key)
            for item in quest.items:
                self.assertGreater(item.obj_id, 0, (quest.key, item.item))
                self.assertGreaterEqual(item.count, 1, (quest.key, item.item))

    def test_known_gates(self):
        blackknight = quest_requirements.by_key("blackknight")
        dragon = quest_requirements.by_key("dragon")
        squire = quest_requirements.by_key("squire")
        self.assertEqual(blackknight.quest_points, 12)
        self.assertEqual(dragon.quest_points, 32)
        self.assertEqual([(skill.skill, skill.level) for skill in squire.skills], [("mining", 10)])


class CheckTest(unittest.TestCase):
    def test_cook_met_from_inventory(self):
        quest = quest_requirements.by_key("cook")
        report = check(quest, state(inventory={1927: 1, 1944: 1, 1933: 1}))
        self.assertTrue(report.met)
        self.assertEqual(report.reasons, ())

    def test_item_missing_reports_each_container(self):
        cook = quest_requirements.by_key("cook")
        report = check(cook, state(inventory={1927: 1}, bank={1933: 1}))
        missing = {reason.item: reason for reason in report.reasons}
        self.assertEqual(sorted(missing), ["egg"])
        self.assertNotIn("bucket_milk", missing)
        self.assertNotIn("pot_flour", missing)

        sheep = quest_requirements.by_key("sheep")
        report = check(sheep, state(inventory={1759: 1}, bank={1759: 4}))
        reason = report.reasons[0]
        self.assertEqual((reason.inventory, reason.bank, reason.owned, reason.required), (1, 4, 5, 20))

    def test_black_knight_armour_must_be_worn(self):
        quest = quest_requirements.by_key("blackknight")
        in_pack = check(quest, state(quest_points=12, inventory={1101: 1, 1139: 1, 1965: 1}))
        self.assertFalse(in_pack.met)
        self.assertEqual([(reason.item, reason.needs_worn) for reason in in_pack.reasons],
                         [("iron_chainbody", True), ("bronze_med_helm", True)])

        worn = check(quest, state(quest_points=12, worn={1101: 1, 1139: 1}, inventory={1965: 1}))
        self.assertTrue(worn.met)

    def test_combat_level_too_low_is_hard(self):
        quest = QuestRequirements(
            key="synthetic_combat",
            name="Synthetic combat",
            members=False,
            completion_varps=((9999, 1),),
            combat_level=50,
        )
        report = check(quest, state(combat_level=3))
        self.assertFalse(report.met)
        self.assertEqual([(reason.TYPE, reason.required, reason.current, reason.soft)
                          for reason in report.reasons],
                         [("combat", 50, 3, False)])

    def test_recommended_combat_is_soft(self):
        quest = QuestRequirements(
            key="synthetic_combat",
            name="Synthetic combat",
            members=False,
            completion_varps=((9999, 1),),
            recommended_combat=20,
        )
        report = check(quest, state(combat_level=3))
        self.assertTrue(report.met)
        self.assertEqual([(reason.TYPE, reason.required, reason.current, reason.soft)
                          for reason in report.reasons],
                         [("combat", 20, 3, True)])

    def test_knights_sword_combat_is_recommended(self):
        quest = quest_requirements.by_key("squire")
        report = check(quest, state(
            levels={"mining": 10},
            inventory={2325: 1, 668: 1, 2351: 2},
            combat_level=3,
        ))
        self.assertTrue(report.met)
        self.assertEqual([(reason.TYPE, reason.soft) for reason in report.reasons], [("combat", True)])

    def test_skill_too_low(self):
        quest = quest_requirements.by_key("squire")
        report = check(quest, state(levels={"mining": 9}, inventory={2325: 1, 668: 1, 2351: 2}, combat_level=99))
        self.assertFalse(report.met)
        self.assertEqual([(reason.TYPE, reason.skill, reason.current) for reason in report.reasons],
                         [("skill", "mining", 9)])

    def test_quest_points_too_low(self):
        quest = quest_requirements.by_key("blackknight")
        report = check(quest, state(quest_points=11, worn={1101: 1, 1139: 1}, inventory={1965: 1}))
        self.assertFalse(report.met)
        self.assertEqual([(reason.TYPE, reason.current, reason.required) for reason in report.reasons],
                         [("quest_points", 11, 12)])

    def test_dependency_not_completed(self):
        dependency = QuestDependency(key="cook", name="Cook's Assistant", completion_varps=((29, 2),))
        quest = QuestRequirements(
            key="synthetic",
            name="Synthetic",
            members=False,
            completion_varps=((9999, 1),),
            dependencies=(dependency,),
        )
        report = check(quest, state())
        self.assertFalse(report.met)
        self.assertEqual([(reason.TYPE, reason.quest) for reason in report.reasons], [("quest", "cook")])
        self.assertTrue(check(quest, state(varps={29: 2})).met)

    def test_completed_quest_is_still_met(self):
        quest = quest_requirements.by_key("cook")
        report = check(quest, state(inventory={1927: 1, 1944: 1, 1933: 1}, varps={29: 2}))
        self.assertTrue(report.met)
        self.assertTrue(report.completed)

    def test_shield_of_arrav_either_gang_completes(self):
        quest = quest_requirements.by_key("blackarmgang")
        self.assertTrue(check(quest, state(inventory={763: 1, 765: 1}, varps={145: 4})).completed)
        self.assertTrue(check(quest, state(inventory={763: 1, 765: 1}, varps={146: 10})).completed)
        self.assertFalse(check(quest, state(inventory={763: 1, 765: 1})).completed)

    def test_report_serialises_typed_reasons(self):
        quest = quest_requirements.by_key("cook")
        payload = check(quest, state()).as_dict()
        self.assertEqual(payload["quest"], "cook")
        self.assertFalse(payload["met"])
        self.assertTrue(all(reason["type"] == "item" for reason in payload["reasons"]))


if __name__ == "__main__":
    unittest.main()
