"""Log several test accounts in at once, prove they can run side by side.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/parallel_login.py [count]
"""

import sys
import threading


from lifescape.client import Bot

results = {}


def run(index):
    username = "t%02d" % (index + 3)
    try:
        bot = Bot()
        bot.login(username)
        bot.say("hello from %s" % username)
        frames = bot.pump(5)
        bot.close()
        results[username] = "ok zone=%s frames=%d" % (bot.zone, len(frames))
    except Exception as error:
        results[username] = "FAIL %s" % error


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    threads = [threading.Thread(target=run, args=(i,)) for i in range(count)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    for username in sorted(results):
        print(username, results[username])
    return 0 if all(value.startswith("ok") for value in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
