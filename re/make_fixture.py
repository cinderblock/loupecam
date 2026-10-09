"""Extract a descrambled control sequence from a capture into a test fixture.

Takes the vendor transfers attributed to one SDK call that follows a mark (see
correlate.py) and writes them as text, one transfer per line:

    in  <req> <wValue> <wIndex> <wLength>
    out <req> <wValue> <wIndex> <data-hex>

wValue/wIndex are the plain (descrambled) values. Fixtures contain no device
identity (serials/flash data come from the open sequence, which is not extracted).

Usage: python re/make_fixture.py <capture> "<mark label>" <SDK fn> <out.txt>
  e.g. python re/make_fixture.py sw_modes "start raw8 size2" Amcam_StartPullModeWithCallback \
       crates/loupecam-protocol/tests/fixtures/start_raw8_1228x922.txt
"""

import json
import sys


def main():
    name, label, fn, out = sys.argv[1:5]
    steps = [json.loads(l) for l in open(f"captures/{name}.jsonl")]
    ctrls = [json.loads(l) for l in open(f"captures/{name}.ctrl.jsonl")]
    ctrls = [c for c in ctrls if c["rt"] & 0x60 == 0x40]
    anchors = [s for s in steps if s["step"] == "mark" or s["step"].startswith("call>")]
    k = next(i for i, s in enumerate(anchors) if s["step"] == "mark" and s["label"] == label)
    k = next(i for i in range(k, len(anchors)) if anchors[i]["step"] == "call>" + fn)
    t0, t1 = anchors[k]["t"] - 0.002, anchors[k + 1]["t"] if k + 1 < len(anchors) else 1e18
    lines = []
    for c in ctrls:
        if not (t0 <= c["t"] < t1):
            continue
        wv, wi = c.get("plain", (c["wValue"], c["wIndex"]))
        if c["dir"] == "in":
            lines.append(f"in  {c['req']:02x} {wv:04x} {wi:04x} {c['wLength']}")
        else:
            lines.append(f"out {c['req']:02x} {wv:04x} {wi:04x} {c['data']}")
    with open(out, "w", newline="\n") as f:
        f.write(f"# {name}: {label} / {fn}\n")
        f.write("\n".join(lines) + "\n")
    print(f"wrote {len(lines)} transfers to {out}")


if __name__ == "__main__":
    main()
