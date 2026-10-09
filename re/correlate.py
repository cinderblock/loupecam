"""Attribute decoded control transfers to the SDK steps that caused them.

Reads captures/<name>.jsonl (sdk_probe step log) and captures/<name>.ctrl.jsonl
(usbpcap.py output, descrambled). Prints, per mark/SDK call, the vendor transfers
that happened after it and before the next one.

Usage: python re/correlate.py <capture-name> [--all]
  --all  also list the high-volume init sequence of each session in full
"""

import json
import sys

AR1820 = {
    0x0100: "mode_select", 0x0104: "grouped_param_hold", 0x0112: "data_format",
    0x0300: "vt_pix_clk_div", 0x0302: "vt_sys_clk_div", 0x0304: "pre_pll_clk_div",
    0x0306: "pll_multiplier", 0x0308: "op_pix_clk_div", 0x030A: "op_sys_clk_div",
    0x0340: "frame_length_lines", 0x0342: "line_length_pck", 0x0344: "x_addr_start",
    0x0346: "y_addr_start", 0x0348: "x_addr_end", 0x034A: "y_addr_end",
    0x034C: "x_output_size", 0x034E: "y_output_size", 0x3012: "coarse_integration_time",
    0x3040: "read_mode", 0x301A: "reset_register", 0x301E: "data_pedestal",
    0x305E: "global_gain", 0x3056: "green1_gain", 0x3058: "blue_gain", 0x305A: "red_gain",
    0x305C: "green2_gain", 0x3070: "test_pattern_mode", 0x30B4: "?30b4",
}


def describe(c):
    req, d = c["req"], c["data"]
    wv, wi = c.get("plain", (c["wValue"], c["wIndex"]))
    if req == 0x0D:
        return f"SENSOR [{wi:04x}] {AR1820.get(wi, '')} = 0x{wv:04x} ({wv})"
    if req == 0x0B:
        return f"FPGA   [{wi >> 8:02x}]{'' if wi & 0xff == 0 else f'.{wi & 0xff:02x}'} = 0x{wv:04x} ({wv})"
    if req == 0x0A:
        return f"READ   wI={wi:04x} wV={wv:04x} -> {d}"
    dirn = "IN " if c["dir"] == "in" else "OUT"
    short = d[:48] + ("…" if len(d) > 48 else "")
    return f"{dirn} req 0x{req:02x} wV={wv:04x} wI={wi:04x} wL={c['wLength']} {short}"


def main():
    name = sys.argv[1]
    show_all = "--all" in sys.argv
    steps = [json.loads(l) for l in open(f"captures/{name}.jsonl")]
    ctrls = [json.loads(l) for l in open(f"captures/{name}.ctrl.jsonl")]
    ctrls = [c for c in ctrls if c["rt"] & 0x60 == 0x40]
    anchors = [s for s in steps if s["step"] in ("mark",) or s["step"].startswith("call>")]
    i = 0
    for k, a in enumerate(anchors):
        t_next = anchors[k + 1]["t"] if k + 1 < len(anchors) else float("inf")
        group = []
        while i < len(ctrls) and ctrls[i]["t"] < t_next:
            if ctrls[i]["t"] >= a["t"] - 0.002:
                group.append(ctrls[i])
            i += 1
        if a["step"] == "mark":
            print(f"\n=== {a['label']}")
            continue
        label = a["step"][5:] + ("(" + ", ".join(map(str, a.get("args", []))) + ")"
                                 if a.get("args") else "")
        if not group:
            print(f"  {label}: (no USB traffic)")
            continue
        print(f"  {label}: {len(group)} transfers")
        lim = len(group) if show_all or len(group) <= 40 else 12
        for c in group[:lim]:
            print("      " + describe(c))
        if lim < len(group):
            print(f"      … {len(group) - lim} more (use --all)")


if __name__ == "__main__":
    main()
