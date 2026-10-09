"""Talk to the camera without the vendor SDK, to test protocol hypotheses.

Usage: re/.venv/Scripts/python re/native_probe.py <experiment>
"""

import os
import sys

import libusb_package
import usb.core

VID, PID = 0x0547, 0x1142
IN, OUT = 0xC0, 0x40


def rotr16(x, n):
    return ((x >> n) | (x << (16 - n))) & 0xFFFF


class Cam:
    def __init__(self, seed=None):
        self.dev = usb.core.find(idVendor=VID, idProduct=PID,
                                 backend=libusb_package.get_libusb1_backend())
        if self.dev is None:
            sys.exit("camera not found")
        self.dev.set_configuration()
        self.key = 0
        if seed is not None:
            self.seed(seed)

    def ctrl_in(self, req, wv, wi, n, scramble=True):
        k = self.key if scramble else 0
        r = bytes(self.dev.ctrl_transfer(IN, req, wv ^ k, wi ^ k, n, timeout=1000))
        print(f"IN  req=0x{req:02x} wV=0x{wv:04x} wI=0x{wi:04x} -> {r.hex()}")
        return r

    def ctrl_out(self, req, wv, wi, data=b"", scramble=True):
        k = self.key if scramble else 0
        n = self.dev.ctrl_transfer(OUT, req, wv ^ k, wi ^ k, data, timeout=1000)
        print(f"OUT req=0x{req:02x} wV=0x{wv:04x} wI=0x{wi:04x} {data.hex()} -> {n}")

    def seed(self, s):
        r = self.ctrl_in(0x16, s, 0, 2, scramble=False)
        self.key = rotr16(s, 4)
        return r


def exp_noauth():
    """Seed, then read chip ID / FPGA version without the challenge-response."""
    c = Cam(seed=int.from_bytes(os.urandom(2), "little"))
    for wi in (0xFFFF, 0xFEFF):
        try:
            c.ctrl_in(0x0A, 0x0000, wi, 2)
        except usb.core.USBError as e:
            print("  error:", e)


def exp_noseed():
    """No seed at all: are reads plain (key 0) or rejected?"""
    c = Cam()
    for wi in (0xFFFF, 0xFEFF):
        try:
            c.ctrl_in(0x0A, 0x0000, wi, 2)
        except usb.core.USBError as e:
            print("  error:", e)


def exp_strings():
    c = Cam()
    for req in (0x1E, 0x1F, 0x10):
        try:
            c.ctrl_in(req, 0, 0, 16, scramble=False)
        except usb.core.USBError as e:
            print("  error:", e)


def load_init(path, stop_after_start=True):
    """Plain control sequence of the first SDK session in a decoded capture:
    from the seed request up to and including the stream-start (req 0x01 wV=3).
    Seed and challenge/response transfers are dropped (we do our own seed and test
    whether auth is needed)."""
    import json
    seq, started, starts = [], False, 0
    for line in open(path):
        c = json.loads(line)
        if c["rt"] & 0x60 != 0x40:
            continue  # standard requests (descriptors, set_config)
        if c["req"] == 0x16:
            if started:
                break
            started = True
            continue
        if not started:
            continue
        if c["wLength"] == 16 and c["req"] in range(0x40, 0x90) and c["req"] not in (0xDA,):
            continue  # challenge/response
        wv, wi = c.get("plain", (c["wValue"], c["wIndex"]))
        seq.append((c["dir"], c["req"], wv, wi, c["wLength"], bytes.fromhex(c["data"]),
                    "plain" in c))
        # 0x01 wV=3 appears right after auth and again at stream start.
        if c["req"] == 0x01 and wv == 3:
            starts += 1
            if stop_after_start and starts == 2:
                break
    return seq


def exp_stream():
    """Replay the SDK's RAW8 1228x922 init without auth and read frames."""
    import time
    seq = load_init("captures/02_stream_controls.ctrl.jsonl")
    c = Cam(seed=int.from_bytes(os.urandom(2), "little"))
    for d, req, wv, wi, wl, data, scr in seq:
        if d == "in":
            r = bytes(c.dev.ctrl_transfer(IN, req, wv ^ (c.key if scr else 0),
                                          wi ^ (c.key if scr else 0), wl, timeout=1000))
            if req not in (0x0B, 0x0D) or r != b"\x08":
                print(f"IN  0x{req:02x} {wv:04x} {wi:04x} -> {r.hex()}")
        else:
            c.dev.ctrl_transfer(OUT, req, wv ^ (c.key if scr else 0),
                                wi ^ (c.key if scr else 0), data, timeout=1000)
    print(f"replayed {len(seq)} transfers; reading bulk")
    frames, cur, t0 = [], bytearray(), time.time()
    while len(frames) < 6 and time.time() - t0 < 10:
        try:
            chunk = bytes(c.dev.read(0x81, 512 * 1024, timeout=2000))
        except usb.core.USBTimeoutError:
            print("bulk timeout")
            continue
        cur += chunk
        if len(chunk) < 512 * 1024:  # short read terminates a frame
            frames.append(bytes(cur))
            print(f"frame {len(frames)}: {len(cur)} bytes, head {cur[:16].hex()} "
                  f"tail {cur[-64:].hex()}")
            cur = bytearray()
    c.dev.ctrl_transfer(IN, 0x0D, 0x0000 ^ c.key, 0x0100 ^ c.key, 1)  # mode_select = standby
    c.dev.ctrl_transfer(OUT, 0x01, 0, 0x0F, b"")  # stream stop
    with open("captures/native_frames.bin", "wb") as f:
        for fr in frames:
            f.write(fr)


if __name__ == "__main__":
    globals()["exp_" + sys.argv[1]]()
