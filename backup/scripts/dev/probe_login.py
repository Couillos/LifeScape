"""Live login probe: validates handshake, CRCs, RSA and ISAAC against
the real server before the SDK builds on them. Exits 0 on login success.

Usage: scripts/orchestrator.py run <account> probe_login [username] [password]
"""

import os
import random
import socket


from lifescape.buffer import Reader, Writer
from lifescape.cache import login_crcs
from lifescape.isaac import Isaac
from lifescape.protocol import SERVER_OPS
from lifescape.rsa import load_private_numbers

HOST = "127.0.0.1"
PORT = 43594
REVISION = 274

HERE = os.path.dirname(os.path.abspath(__file__))
SERVER_KEY = os.path.join(HERE, "..", "..", "lostcity", "engine", "data", "config", "private.pem")
PACK_DIR = os.path.join(HERE, "..", "..", "lostcity", "engine", "data", "pack")

RESPONSES = {
    1: "invalid credentials",
    2: "ok",
    3: "invalid login packet",
    5: "already logged in",
    6: "client out of date (rev, crc or rsa mismatch)",
    7: "world full",
    16: "rate limited",
}


def read_exact(sock, length):
    out = bytearray()
    while len(out) < length:
        chunk = sock.recv(length - len(out))
        if not chunk:
            raise ConnectionError("server closed the socket")
        out += chunk
    return bytes(out)


def run(bot, *args):
    username = args[0] if len(args) > 0 else "bite"
    password = args[1] if len(args) > 1 else username
    return probe(username, password)


def probe(username, password):
    seeds = [random.randrange(0, 0x7FFFFFFF) for _ in range(4)]
    uid = random.randrange(0, 0x7FFFFFFF)

    sock = socket.create_connection((HOST, PORT), timeout=10)

    sock.sendall(bytes([14, 0]))
    session = read_exact(sock, 17)
    if session[8] != 0:
        raise ValueError("unexpected handshake reply: %r" % (session,))

    rsa_plain = Writer()
    rsa_plain.u8(10)
    for seed in seeds:
        rsa_plain.i32(seed)
    rsa_plain.i32(uid)
    rsa_plain.jstr(username)
    rsa_plain.jstr(password)

    key = load_private_numbers(SERVER_KEY)
    cipher = key.encrypt(rsa_plain.bytes())

    login = Writer()
    login.u8(0xFF)
    login.u16(REVISION)
    login.u8(0)
    login.raw(login_crcs(os.path.normpath(PACK_DIR)))
    login.u8(len(cipher))
    login.raw(cipher)
    payload = login.bytes()

    sock.sendall(bytes([16, len(payload)]) + payload)
    open("/tmp/opencode/login.bin", "wb").write(payload)
    print("payload len=%d cipher len=%d" % (len(payload), len(cipher)))

    response = read_exact(sock, 1)[0]
    print("login response: %d (%s)" % (response, RESPONSES.get(response, "unknown")))
    if response != 2:
        return 1

    staff, flag = read_exact(sock, 2)
    print("staff=%d flag=%d seeds=%s" % (staff, flag, seeds))

    decryptor = Isaac([seed + 50 for seed in seeds])
    sock.settimeout(10)
    for _ in range(6):
        opcode = (read_exact(sock, 1)[0] - decryptor.next_int()) & 0xFF
        name, length = SERVER_OPS.get(opcode, ("UNKNOWN", None))
        if length == -1:
            length = read_exact(sock, 1)[0]
        elif length == -2:
            length = int.from_bytes(read_exact(sock, 2), "big")
        elif length is None:
            print("unknown opcode %d, stopping" % opcode)
            return 1
        body = read_exact(sock, length)
        print("recv %s (%d) len=%d body=%s" % (name, opcode, length, body.hex()))
        if name == "REBUILD_NORMAL":
            zone = Reader(body)
            print("spawn zone: x=%d z=%d" % (zone.u16(), zone.u16()))
    return 0
