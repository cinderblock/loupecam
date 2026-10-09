"""Minimal USBPcap (pcap linktype 249) reader for reverse-engineering the camera.

Pairs control-transfer setup packets with their completions and prints one line
per transfer. Bulk traffic is summarised per URB.

Usage: python re/usbpcap.py <file.pcap> [--bulk] [--jsonl out.jsonl]
"""

import json
import struct
import sys
from dataclasses import dataclass, field

FUNC_CONTROL = 0x08
XFER_ISO, XFER_INT, XFER_CTRL, XFER_BULK = 0, 1, 2, 3


@dataclass
class Pkt:
    n: int
    t: float
    irp: int
    status: int
    function: int
    completion: bool  # info bit 0: 1 = PDO->FDO (completion)
    bus: int
    dev: int
    ep: int
    xfer: int
    data_len: int
    stage: int | None
    payload: bytes
    caplen_short: bool = False


def read_pcap(path):
    with open(path, "rb") as f:
        gh = f.read(24)
        magic = struct.unpack("<I", gh[:4])[0]
        if magic not in (0xA1B2C3D4, 0xA1B23C4D):
            raise ValueError("not a little-endian pcap")
        nano = magic == 0xA1B23C4D
        linktype = struct.unpack("<I", gh[20:24])[0]
        if linktype != 249:
            raise ValueError(f"linktype {linktype} is not USBPcap")
        n = 0
        while True:
            rh = f.read(16)
            if len(rh) < 16:
                return
            sec, frac, incl, orig = struct.unpack("<IIII", rh)
            body = f.read(incl)
            n += 1
            hlen, irp, status, function, info, bus, dev, ep, xfer, dlen = struct.unpack(
                "<HQIHBHHBBI", body[:27])
            stage = body[27] if xfer == XFER_CTRL and hlen >= 28 else None
            yield Pkt(n, sec + frac / (1e9 if nano else 1e6), irp, status, function,
                      bool(info & 1), bus, dev, ep, xfer, dlen, stage, body[hlen:],
                      incl < orig)


@dataclass
class Ctrl:
    t: float
    bmRequestType: int
    bRequest: int
    wValue: int
    wIndex: int
    wLength: int
    out_data: bytes = b""
    in_data: bytes = b""
    status: int | None = None
    t_done: float | None = None

    @property
    def is_in(self):
        return bool(self.bmRequestType & 0x80)

    def line(self):
        d = self.in_data if self.is_in else self.out_data
        st = "" if not self.status else f" status=0x{self.status:08x}"
        return (f"{self.t:.6f} {'IN ' if self.is_in else 'OUT'} rt=0x{self.bmRequestType:02x} "
                f"req=0x{self.bRequest:02x}({self.bRequest:3d}) wV=0x{self.wValue:04x} "
                f"wI=0x{self.wIndex:04x} wL={self.wLength:<5d} [{len(d)}] {d[:64].hex()}"
                f"{'…' if len(d) > 64 else ''}{st}")

    def as_dict(self):
        d = {"t": self.t, "rt": self.bmRequestType, "req": self.bRequest, "wValue": self.wValue,
             "wIndex": self.wIndex, "wLength": self.wLength, "dir": "in" if self.is_in else "out",
             "data": (self.in_data if self.is_in else self.out_data).hex(), "status": self.status}
        if hasattr(self, "plain"):
            d["plain"] = list(self.plain)
        return d


def transfers(path, show_bulk=False):
    pending: dict[int, Ctrl] = {}
    bulk_pending: dict[int, Pkt] = {}
    for p in read_pcap(path):
        if p.xfer == XFER_CTRL:
            if not p.completion and p.stage == 0 and len(p.payload) >= 8:
                rt, rq, wv, wi, wl = struct.unpack("<BBHHH", p.payload[:8])
                c = Ctrl(p.t, rt, rq, wv, wi, wl)
                c.out_data = p.payload[8:]
                pending[p.irp] = c
            elif not p.completion and p.stage == 1 and p.irp in pending:
                pending[p.irp].out_data += p.payload
            elif p.completion and p.irp in pending:
                c = pending[p.irp]
                c.in_data += p.payload
                if p.stage in (2, 3) or p.stage is None or c.is_in:
                    if p.stage == 3 or not c.is_in or p.stage == 1 or p.payload:
                        pass
                c.status = p.status
                c.t_done = p.t
                if p.stage != 1 or True:
                    yield ("ctrl", pending.pop(p.irp))
        elif p.xfer == XFER_BULK:
            if not p.completion:
                bulk_pending[p.irp] = p
                if p.ep & 0x80 == 0 and show_bulk:
                    yield ("bulk", p)
            else:
                req = bulk_pending.pop(p.irp, None)
                if show_bulk and p.ep & 0x80:
                    yield ("bulk", p)


SEED_REQ = 0x16
SCRAMBLED_REQS = {0x0A, 0x0B, 0x0D}


def rotr16(x, n):
    return ((x >> n) | (x << (16 - n))) & 0xFFFF


def main():
    path = sys.argv[1]
    show_bulk = "--bulk" in sys.argv
    out = None
    if "--jsonl" in sys.argv:
        out = open(sys.argv[sys.argv.index("--jsonl") + 1], "w")
    key = 0
    for kind, x in transfers(path, show_bulk):
        if kind == "ctrl":
            plain = ""
            if x.bmRequestType & 0x60 == 0x40:  # vendor
                if x.bRequest == SEED_REQ:
                    key = rotr16(x.wValue, 4)
                    plain = f"  ; seed -> key 0x{key:04x}"
                elif x.bRequest in SCRAMBLED_REQS:
                    x.plain = (x.wValue ^ key, x.wIndex ^ key)
                    plain = f"  ; plain wV=0x{x.plain[0]:04x} wI=0x{x.plain[1]:04x}"
            print(x.line() + plain)
            if out:
                out.write(json.dumps(x.as_dict()) + "\n")
        else:
            print(f"{x.t:.6f} BULK ep=0x{x.ep:02x} len={x.data_len} st=0x{x.status:08x} "
                  f"{x.payload[:32].hex()}")


if __name__ == "__main__":
    main()
