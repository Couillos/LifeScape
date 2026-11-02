"""Create test accounts with realistic random names and randomized looks.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/make_accounts.py [count]

Password is the username (test convention). For each account: pick a
unique realistic name, store it with a server-compatible bcrypt hash,
log in, send a random IDK_SAVEDESIGN while the tutorial design screen
is open, accept it, log out, then prove the .sav on disk holds the
look that was sent.
"""

import json
import os
import random
import sqlite3
import subprocess
import sys
import time


from lifescape.client import Bot
from lifescape.world import to_base37

from orchestration.save_reader import look as saved_look
from scripts.tutorial.common import COM_DESIGN_ACCEPT, COM_DESIGN_MAIN, wait_modal_closed

HERE = os.path.dirname(os.path.abspath(__file__))
ENGINE_DIR = os.path.normpath(os.path.join(HERE, "..", "..", "lostcity", "engine"))
DB_PATH = os.path.join(ENGINE_DIR, "db.sqlite")
BCRYPT_NODE_PATH = "/opt/server/engine/node_modules/bcrypt-ts/dist/node.js"

# Server names are base37, 12 chars max (512-bit RSA login packet).
MAX_NAME_LENGTH = 12

# Valid kits per body slot, from the IdkType dump (male types 0-6,
# female types 7-13, female jaw has no kit so it is always -1).
MALE_KITS = [range(0, 9), range(10, 18), range(18, 26), range(26, 32), (33, 34), range(36, 41), (42, 43)]
FEMALE_KITS = [range(45, 55), (-1,), range(56, 61), range(61, 66), (67, 68), range(70, 78), (79, 80)]

# Valid color indexes per slot, from Player.DESIGN_BODY_COLORS lengths.
COLOR_COUNTS = (12, 16, 16, 6, 8)

FIRST_NAMES = [
    "adam", "alex", "amy", "anna", "ben", "beth", "carl", "chris",
    "dan", "dave", "ella", "emma", "eric", "fred", "gina", "harry",
    "helen", "jack", "james", "jane", "jason", "jenny", "joe",
    "john", "kate", "karl", "kevin", "kim", "laura", "leo", "lisa",
    "lucy", "mike", "nick", "nina", "olivia", "oscar", "paul",
    "peter", "rachel", "ryan", "sam", "sara", "simon", "steve",
    "susan", "tom", "tony", "victor", "wendy", "will", "zoe",
]

LAST_NAMES = [
    "adams", "allen", "baker", "bell", "brown", "carter", "clark",
    "cole", "cook", "cooper", "davis", "dean", "edwards", "ellis",
    "evans", "fisher", "ford", "foster", "fox", "gray", "green",
    "hall", "harris", "hill", "holmes", "hunter", "jones", "kelly",
    "king", "lane", "lee", "lewis", "marsh", "mason", "mills",
    "moore", "morgan", "nash", "nelson", "owen", "parker", "payne",
    "price", "reed", "shaw", "smith", "stone", "taylor", "turner",
    "walker", "ward", "webb", "white", "wood", "young",
]


def load_existing():
    connection = sqlite3.connect(DB_PATH)
    try:
        rows = connection.execute("SELECT username FROM account").fetchall()
    finally:
        connection.close()
    names = set(row[0] for row in rows)
    return names, set(to_base37(name) for name in names)


def build_username(first_name, last_name):
    return ("%s_%s" % (first_name, last_name))[:MAX_NAME_LENGTH].rstrip("_")


def random_username(taken_names, taken_identities, rng):
    for _attempt in range(100):
        candidate = build_username(rng.choice(FIRST_NAMES), rng.choice(LAST_NAMES))
        if len(candidate) >= 3 and candidate not in taken_names and to_base37(candidate) not in taken_identities:
            return candidate
    for _attempt in range(100):
        candidate = build_username(rng.choice(FIRST_NAMES), rng.choice(LAST_NAMES))[:MAX_NAME_LENGTH - 1]
        candidate = (candidate + str(rng.randrange(10)))[:MAX_NAME_LENGTH]
        if len(candidate) >= 3 and candidate not in taken_names and to_base37(candidate) not in taken_identities:
            return candidate
    raise RuntimeError("no free username after 200 tries")


def random_look(rng):
    # Three quarters male: most test avatars read as the default male body.
    gender = 0 if rng.random() < 0.75 else 1
    table = MALE_KITS if gender == 0 else FEMALE_KITS
    kits = [rng.choice(list(slot)) for slot in table]
    colors = [rng.randrange(count) for count in COLOR_COUNTS]
    return gender, kits, colors


def hash_passwords(passwords):
    program = (
        "import * as bcrypt from '" + BCRYPT_NODE_PATH + "';"
        "import fs from 'node:fs';"
        "const words = JSON.parse(fs.readFileSync(0, 'utf8'));"
        "const out = {};"
        "for (const word of words) { out[word] = await bcrypt.hash(word.toLowerCase(), 10); }"
        "process.stdout.write(JSON.stringify(out));"
    )
    try:
        completed = subprocess.run(
            ["docker", "exec", "-i", "lostcity", "node", "--input-type=module", "-e", program],
            input=json.dumps(passwords).encode("utf-8"),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30 + 10 * len(passwords),
        )
    except FileNotFoundError:
        raise SystemExit("docker CLI not found, cannot hash passwords")
    if completed.returncode != 0:
        raise SystemExit("hashing failed: %s" % completed.stderr.decode("utf-8", "replace")[-500:])
    return json.loads(completed.stdout.decode("utf-8"))


def store_account(username, password_hash):
    connection = sqlite3.connect(DB_PATH, timeout=30)
    try:
        for _attempt in range(5):
            try:
                connection.execute(
                    "INSERT INTO account(username, password) VALUES(?, ?)", (username, password_hash)
                )
                connection.commit()
                return
            except sqlite3.OperationalError as error:
                if "locked" not in str(error):
                    raise
                time.sleep(2)
    finally:
        connection.close()
    raise SystemExit("database locked, could not store %s" % username)


def apply_random_look(username, gender, kits, colors):
    bot = Bot()
    try:
        bot.login(username, username)
        deadline = time.time() + 30
        while time.time() < deadline and bot.world.modal is None:
            bot.pump(5)
        if bot.world.modal != ("main", COM_DESIGN_MAIN):
            raise SystemExit("no design screen for %s (modal=%s)" % (username, bot.world.modal))
        bot.save_design(gender, kits, colors)
        bot.if_button(COM_DESIGN_ACCEPT)
        if not wait_modal_closed(bot):
            raise SystemExit("design not accepted for %s" % username)
    finally:
        bot.close()


def await_saved_look(username, gender, kits, colors):
    deadline = time.time() + 120
    while time.time() < deadline:
        if saved_look(username) == (gender, kits, colors):
            return True
        time.sleep(2)
    return False


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    taken_names, taken_identities = load_existing()
    fresh_names = []
    for _slot in range(count):
        username = random_username(taken_names, taken_identities, random)
        taken_names.add(username)
        taken_identities.add(to_base37(username))
        fresh_names.append(username)
    hashes = hash_passwords(fresh_names)
    for username in fresh_names:
        store_account(username, hashes[username])
        gender, kits, colors = random_look(random)
        print("sent %s look=%s" % (username, [gender] + kits + colors), flush=True)
        apply_random_look(username, gender, kits, colors)
        if not await_saved_look(username, gender, kits, colors):
            raise SystemExit("saved look mismatch for %s" % username)
        print("ok %s look=%s" % (username, [gender] + kits + colors), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
