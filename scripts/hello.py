"""Log in, say hello, show what the server sends, leave.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/hello.py [username] [password]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape.client import Bot

MESSAGE_GAME = 161


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t02"
    password = sys.argv[2] if len(sys.argv) > 2 else username
    bot = Bot()
    try:
        bot.login(username, password)
        print("logged in as %s in zone %s" % (username, bot.zone))
        bot.say("hello lifescape")
        bot.idle()
        for opcode, name, body in bot.pump(25):
            if opcode == MESSAGE_GAME:
                print("game message: %s" % body[:-1].decode("latin-1"))
            else:
                print("recv %s (%d) len=%d" % (name, opcode, len(body)))
    finally:
        bot.close()


if __name__ == "__main__":
    main()
