"""scripts/ ne contient que du gameplay : skills, quetes, random, rescues.

Les outils dev (probe_*, check_*, dump_*, goto/walkto...) vivent dans
scripts/dev/ et le registre pointe dessus. Ce test verrouille la regle.
"""

import os
import unittest

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "scripts"))

ALLOWED_TOP = {"__init__.py", "orchestrator.py", "randomevents.py", "hazards.py"}
ALLOWED_PREFIXES = ("rescue_",)


class ScriptsLayoutTest(unittest.TestCase):
    def test_top_level_is_gameplay_only(self):
        top = sorted(name for name in os.listdir(ROOT)
                     if name.endswith(".py") and os.path.isfile(os.path.join(ROOT, name)))
        intruders = [name for name in top
                     if name not in ALLOWED_TOP and not name.startswith(ALLOWED_PREFIXES)]
        self.assertEqual(intruders, [])

    def test_dev_tools_are_registered_there(self):
        import orchestration.registry as registry
        for name, path in registry.SCRIPTS.items():
            if name in ("fishing", "mining", "woodcut", "melee", "tutorial") or name.startswith("quest_"):
                continue
            if name.startswith("rescue_"):
                self.assertTrue(path.startswith("scripts/rescue_"), (name, path))
            else:
                self.assertTrue(path.startswith("scripts/dev/"), (name, path))


if __name__ == "__main__":
    unittest.main()
