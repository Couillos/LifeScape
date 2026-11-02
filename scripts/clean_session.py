"""Free an account session after a crash: log in, then log out cleanly.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/clean_session.py <account>

The orchestrator runs this before relaunching a crashed job. The server
keeps a session for a few ticks after a hard kill, so this retries until
the login is accepted, then closes it at once.
"""

import sys
import time


from lifescape.client import Bot

SESSION_WAIT_SECONDS = 30


def main():
    account = sys.argv[1] if len(sys.argv) > 1 else ""
    if not account:
        print("usage: clean_session.py <account>", file=sys.stderr)
        return 2
    deadline = time.time() + SESSION_WAIT_SECONDS
    while True:
        bot = Bot()
        try:
            bot.login(account, account)
            bot.intend("session cleanup")
            bot.pump(5)
            print("session freed for %s" % account, flush=True)
            return 0
        except Exception as error:
            print("session busy for %s: %s" % (account, error), flush=True)
        finally:
            try:
                bot.close()
            except Exception:
                pass
        if time.time() > deadline:
            print("session still busy for %s" % account, flush=True)
            return 1
        # Process-level retry, not game pacing: the bot is not in the world.
        time.sleep(2)


if __name__ == "__main__":
    sys.exit(main())
