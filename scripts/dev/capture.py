"""Capture raw server frames to a file for offline decoder work.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/dev/capture.py [username] [frames] [outfile]
"""

import json
import sys


from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t08"
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 200
    outfile = sys.argv[3] if len(sys.argv) > 3 else "capture_%s.json" % username
    bot = Bot()
    try:
        bot.login(username)
        captured = []
        for opcode, name, body in bot.pump(count):
            captured.append({"op": opcode, "name": name, "hex": body.hex()})
        with open(outfile, "w") as handle:
            json.dump(captured, handle)
        print("wrote %d frames to %s" % (len(captured), outfile))
    finally:
        bot.close()


if __name__ == "__main__":
    main()
