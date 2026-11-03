"""Socket, login handshake and framed opcodes for revision 274."""

import errno
import os
import random
import select
import socket
import time
from .buffer import Writer
from .cache import login_crcs
from .isaac import Isaac
from .protocol import ALL_SERVER_OPS, CLIENT_OPS
from .rsa import load_private_numbers


class LoginError(Exception):
    pass


RESPONSES = {
    1: "invalid credentials",
    2: "ok",
    3: "invalid login packet",
    5: "already logged in",
    6: "client out of date",
    7: "world full",
    16: "rate limited",
}


class Connection:
    def __init__(self, host="127.0.0.1", port=43594, timeout=10):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock = None
        self.outgoing = None
        self.incoming = None
        self.trace = None
        self.last_send = 0.0
        self.packet_index = 0
        self._buffer = bytearray()

    def connect(self):
        self.sock = socket.create_connection((self.host, self.port), self.timeout)
        # Short slices so a signal (kill/^C) is handled promptly instead of
        # waiting out a blocking recv; _read_exact still enforces self.timeout.
        self.sock.settimeout(0.5)

    def close(self):
        if self.sock is not None:
            self.sock.close()
            self.sock = None

    def login(self, username, password, pack_dir, server_key_path, revision=274):
        seeds = [random.randrange(0, 0x7FFFFFFF) for _ in range(4)]
        uid = random.randrange(0, 0x7FFFFFFF)

        self._send_raw(bytes([14, 0]))
        if self._read_exact(8) != bytes(8) or self._read_exact(1) != bytes([0]):
            raise LoginError("bad handshake reply")
        self._read_exact(8)

        rsa_plain = Writer()
        rsa_plain.u8(10)
        for seed in seeds:
            rsa_plain.i32(seed)
        rsa_plain.i32(uid)
        rsa_plain.jstr(username)
        rsa_plain.jstr(password)

        key = load_private_numbers(server_key_path)
        cipher = key.encrypt(rsa_plain.bytes())

        login = Writer()
        login.u8(0xFF)
        login.u16(revision)
        login.u8(0)
        login.raw(login_crcs(pack_dir))
        login.u8(len(cipher))
        login.raw(cipher)
        payload = login.bytes()
        self._send_raw(bytes([16, len(payload)]) + payload)

        response = self._read_exact(1)[0]
        if response != 2:
            raise LoginError("login refused: %d (%s)" % (response, RESPONSES.get(response, "unknown")))

        staff, flag = self._read_exact(2)
        self.outgoing = Isaac(seeds)
        self.incoming = Isaac([seed + 50 for seed in seeds])
        self.seeds = seeds
        return {"staff": staff, "flag": flag, "seeds": seeds, "uid": uid}

    def send(self, op_id, payload=b""):
        name, size = CLIENT_OPS[op_id]
        if size == -1:
            frame = bytes([len(payload)]) + payload
        elif size == -2:
            frame = len(payload).to_bytes(2, "big") + payload
        else:
            if len(payload) != size:
                raise ValueError("%s needs %d bytes, got %d" % (name, size, len(payload)))
            frame = payload
        self._send_raw(bytes([(op_id + self.outgoing.next_int()) & 0xFF]) + frame)
        self.last_send = time.time()
        if self.trace is not None:
            self.trace.write("sent %d %s %d\n" % (op_id, name, len(frame)))

    def recv(self):
        raw = self._read_exact(1)[0]
        shift = self.incoming.next_int()
        opcode = (raw - shift) & 0xFF
        self.packet_index += 1
        if self.trace is not None:
            self.trace.write("%d %d %d %d\n" % (self.packet_index, raw, shift, opcode))
        if opcode not in ALL_SERVER_OPS:
            raise LoginError("unknown server opcode %d" % opcode)
        name, size = ALL_SERVER_OPS[opcode]
        if size == -1:
            size = self._read_exact(1)[0]
        elif size == -2:
            size = int.from_bytes(self._read_exact(2), "big")
        body = self._read_exact(size) if size else b""
        if self.trace is not None:
            self.trace.write("frame %d %s %d %s\n" % (opcode, name, size, body.hex()[:96]))
        return opcode, name, body

    def _send_raw(self, data):
        self.sock.sendall(data)

    def available(self):
        """True when the server already sent more data than we read.

        Non-blocking on purpose: a client that waits here falls behind the
        tick and stops sending keepalives, so the server drops it.
        """
        if self._buffer:
            return True
        if self.sock is None:
            return False
        readable, _writable, _error = select.select([self.sock], [], [], 0)
        if not readable:
            return False
        try:
            chunk = self.sock.recv(65536)
        except OSError as error:
            # select can report readable and the recv still find nothing
            # (spurious wakeup): only EAGAIN is benign, anything else raises.
            if error.errno not in (errno.EAGAIN, errno.EWOULDBLOCK):
                raise
            return False
        if not chunk:
            raise ConnectionError("server closed the socket")
        self._buffer += chunk
        return True

    def _read_exact(self, length):
        out = bytearray()
        deadline = time.time() + self.timeout
        while len(out) < length:
            if self._buffer:
                take = min(length - len(out), len(self._buffer))
                out += self._buffer[:take]
                del self._buffer[:take]
                continue
            try:
                chunk = self.sock.recv(length - len(out))
            except socket.timeout:
                if time.time() > deadline:
                    raise
                continue
            except OSError as error:
                # Same spurious wakeup as available(): a timeout socket can
                # surface EAGAIN instead of socket.timeout.
                if error.errno not in (errno.EAGAIN, errno.EWOULDBLOCK):
                    raise
                if time.time() > deadline:
                    raise TimeoutError("read timed out")
                continue
            if not chunk:
                raise ConnectionError("server closed the socket")
            out += chunk
        return bytes(out)


def default_paths():
    here = os.path.dirname(os.path.abspath(__file__))
    engine = os.path.normpath(os.path.join(here, "..", "..", "lostcity", "engine"))
    return {
        "pack_dir": os.path.join(engine, "data", "pack"),
        "server_key": os.path.join(engine, "data", "config", "private.pem"),
        "types": os.path.normpath(os.path.join(here, "data", "gametypes.json")),
        "comps": os.path.normpath(os.path.join(here, "..", "gamedata", "data", "comps.json")),
    }
