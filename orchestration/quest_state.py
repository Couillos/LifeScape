"""Quest completion read by the orchestrator, never by a bot script."""

import gamedata as knowledge

from lifescape import save


def completion(account, quest_key):
    """True/False from the saved quest varps, or None when there is no save."""
    try:
        requirements = knowledge.quest_requirements_by_key(quest_key)
    except KeyError:
        return None
    saved = save.load_account(account)
    varps = saved.get("varps")
    if varps is None:
        return None
    return any(varps.get(varp_id) == value for varp_id, value in requirements.completion_varps)
