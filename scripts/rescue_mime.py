"""Rescue a bot stuck in the Mime random event.

Mechanics (content/scripts/macro events/scripts/general/macro_event_mime.rs2):
every cycle the mime plays one of 8 emotes (%npc_int = random(8)), then the
emotes interface opens and the player must click the matching button, 4
correct in a row to be teleported back. A wrong click only resets the
progress, so the worst case here is slowness, never damage.

The bot cannot read %npc_int (server-side), but the emote arrives as the
mime NPC's anim mask (0x2: u16 seq + u8 delay, parsed into
world.npcs[nid]["anim"]). Capture the emote while it plays, click when the
interface opens: %npc_int only changes when the next emote plays.

Usage: scripts/orchestrator.py run <account> rescue_mime
"""

from scripts import randomevents
from scripts.randomevents import in_mime

# comps.json: 6543 is the interface, com_0 is 6544, so com_2 is 6546.
# The rs2 maps com_2 to emote 0 (cry) through com_9, emote 7 (glass box).
EMOTE_COM_BASE = 6546

# seq.pack: name -> seq id, the anim the mime NPC plays.
SEQ_TO_EMOTE = {
    860: 0,  # emote_cry
    857: 1,  # emote_think
    861: 2,  # emote_laugh
    866: 3,  # emote_dance
    1130: 4,  # emote_climbing_rope
    1129: 5,  # emote_mime_lean
    1128: 6,  # emote_glass_wall
    1131: 7,  # emote_glass_box
}

MIME_MODAL = ("chat", 6543)

# A cycle is 32 ticks; 4 correct in a row, plus slack for a slow link and
# for arriving mid-cycle with the emote already missed.
WAIT_TICKS = 2000


def button_for(seq):
    """Interface com that copies the given emote seq id."""
    return EMOTE_COM_BASE + SEQ_TO_EMOTE[seq]


def mime_anim(bot):
    """Current anim seq of the nearest mime NPC, or None when unseen."""
    try:
        slot = bot.find_npc("macro_mime")
    except Exception:
        return None
    if slot is None:
        return None
    return (bot.world.npcs.get(slot) or {}).get("anim")


def _verify(bot):
    """0 when the bot is truly free, else report the pending event (1)."""
    event = randomevents.event_at(bot)
    if event is None:
        print("mime rescue done, free at %s" % (bot.position,), flush=True)
        return 0
    randomevents.report(bot, event, False)
    print("mime rescue handed off to %s event at %s" % (event, bot.position,), flush=True)
    return 1


def run(bot, *args):
    bot.pump(10)
    bot.intend("rescue: mime")
    print("start %s" % (bot.position,), flush=True)
    if not in_mime(bot):
        print("not in mime, nothing to do", flush=True)
        return _verify(bot)
    last_anim = mime_anim(bot)
    captured = None
    deadline = bot.world.ticks + WAIT_TICKS
    while in_mime(bot) and bot.world.ticks < deadline:
        bot.pump(1)
        anim = mime_anim(bot)
        if anim != last_anim:
            last_anim = anim
            if anim in SEQ_TO_EMOTE:
                captured = anim
        if bot.world.modal == MIME_MODAL and captured is not None:
            print("copying emote %d (seq %d)" % (SEQ_TO_EMOTE[captured], captured), flush=True)
            bot.if_button(button_for(captured))
            captured = None
            bot.pump(2)
    if in_mime(bot):
        print("mime still active after %d ticks" % WAIT_TICKS, flush=True)
        return 1
    print("mime solved, back at %s" % (bot.position,), flush=True)
    return _verify(bot)
