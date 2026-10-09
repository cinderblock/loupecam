"""Compare the stream-start register programming across sessions of one capture.

For every "start ..." mark, collects the final value written to each sensor/FPGA
register during that start, then prints the registers whose values differ between
starts. Also prints the order of the first start in full, for reference.

Usage: python re/diff_modes.py <capture-name>
"""

import json
import sys

from correlate import AR1820


def main():
    name = sys.argv[1]
    steps = [json.loads(l) for l in open(f"captures/{name}.jsonl")]
    ctrls = [json.loads(l) for l in open(f"captures/{name}.ctrl.jsonl")]
    ctrls = [c for c in ctrls if c["rt"] & 0x60 == 0x40]
    marks = [s for s in steps if s["step"] == "mark" and s["label"].startswith("start")]
    stops = [s for s in steps if s["step"] == "call>Amcam_Stop"]
    table, other = {}, {}
    for m in marks:
        end = next(s["t"] for s in stops if s["t"] > m["t"])
        regs, misc = {}, []
        for c in ctrls:
            if m["t"] <= c["t"] < end:
                wv, wi = c.get("plain", (c["wValue"], c["wIndex"]))
                if c["req"] == 0x0D:
                    regs[f"S{wi:04x}"] = wv
                elif c["req"] == 0x0B:
                    regs[f"F{wi >> 8:02x}"] = wv
                elif c["req"] in (0xD9, 0xDA):
                    regs[f"R{c['req']:02x}.{wi:04x}"] = c["data"][:16]
                else:
                    misc.append(f"{c['req']:02x}:{wv:04x}:{wi:04x}")
        table[m["label"]] = regs
        other[m["label"]] = misc
    keys = sorted({k for r in table.values() for k in r})
    labels = list(table)
    short = [l.replace("start ", "") for l in labels]
    print("reg".ljust(22) + "".join(s[:12].rjust(13) for s in short))
    for k in keys:
        vals = [table[l].get(k) for l in labels]
        if len(set(map(str, vals))) == 1:
            continue
        nm = AR1820.get(int(k[1:], 16), "") if k[0] == "S" else ""
        cells = "".join((f"{v:#06x}" if isinstance(v, int) else (v or "-")[:12]).rjust(13)
                        for v in vals)
        print(f"{k} {nm}"[:22].ljust(22) + cells)
    print("\nconstant across all starts:")
    for k in keys:
        vals = [table[l].get(k) for l in labels]
        if len(set(map(str, vals))) == 1 and isinstance(vals[0], int):
            nm = AR1820.get(int(k[1:], 16), "") if k[0] == "S" else ""
            print(f"  {k} {nm} = {vals[0]:#06x}")
    print("\nother requests per start:")
    for l in labels:
        print(f"  {l}: {' '.join(other[l])}")


if __name__ == "__main__":
    main()
