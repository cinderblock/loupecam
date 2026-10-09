"""Generate the SDK sweep step files used with re/capture.ps1.

Run: python re/sweeps/gen.py   (writes re/sweeps/*.json)
"""

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

OPT_RAW, OPT_BITDEPTH, OPT_TRIGGER = 0x04, 0x06, 0x0B
OPT_LINEAR, OPT_CURVE, OPT_COLORMATIX, OPT_WBGAIN = 0x09, 0x0A, 0x0D, 0x0E
OPT_FRAMERATE, OPT_BLACKLEVEL, OPT_CG, OPT_TESTPATTERN = 0x11, 0x15, 0x19, 0x28
OPT_BANDWIDTH, OPT_PRECISE_FRAMERATE, OPT_LOW_NOISE = 0x2E, 0x2D, 0x38
OPT_DEFECT_PIXEL, OPT_HIGH_FULLWELL, OPT_ISP, OPT_PIXEL_FORMAT = 0x40, 0x55, 0x5F, 0x1A
OPT_DENOISE, OPT_SHARPENING, OPT_FPNC, OPT_ZERO_OFFSET = 0x35, 0x1E, 0x67, 0x70
OPT_UPTIME, OPT_LINE_TIME, OPT_MAX_PRECISE_FRAMERATE = 0x79, 0x77, 0x2C


def mark(label):
    return {"mark": label}


def fn(name, *args):
    return {"fn": name, "args": list(args)}


def opt(o, v):
    return {"option": o, "value": v}


def P(s=0.8):
    return {"pause": s}


def session(fmt, size, body, settle=1.5):
    """fmt: raw8 | raw16 | rgb24 | rgb48"""
    raw = fmt.startswith("raw")
    deep = fmt in ("raw16", "rgb48")
    bits = {"raw8": 8, "raw16": 16, "rgb24": 24, "rgb48": 48}[fmt]
    steps = [{"open": True}, {"info": True},
             opt(OPT_RAW, 1 if raw else 0), opt(OPT_BITDEPTH, 1 if deep else 0)]
    if fmt == "rgb48":
        steps.append(opt(0x0C, 1))  # OPTION_RGB = RGB48
    steps += [fn("put_eSize", size), {"pullbits": bits},
              fn("put_AutoExpoEnable", 0), fn("put_ExpoTime", 20000), fn("put_ExpoAGain", 100),
              mark(f"start {fmt} size{size}"), {"start": True}, P(settle)]
    steps += body
    steps += [{"stop": True}, {"close": True}]
    return steps


def modes():
    steps = []
    for size in (0, 1, 2):
        for fmt in ("raw8", "raw16", "rgb24"):
            steps += session(fmt, size, [{"save": f"mode_{fmt}_s{size}"}],
                             settle=3.0 if size == 0 else 1.5)
    return steps


def sensor_controls():
    b = [mark("baseline"), P(1)]
    for us in (100, 1000, 5000, 33333, 100000, 500000):
        b += [mark(f"expo {us}"), fn("put_ExpoTime", us), P(0.6 + us / 1e6)]
    b += [fn("put_ExpoTime", 20000), P()]
    for g in (100, 150, 200, 300, 400, 500):
        b += [mark(f"gain {g}"), fn("put_ExpoAGain", g), P()]
    b += [fn("put_ExpoAGain", 100), P()]
    for sp in (0, 1, 2, 3):
        b += [mark(f"speed {sp}"), fn("put_Speed", sp), P(1.2)]
    for hz in (0, 1, 2):
        b += [mark(f"hz {hz}"), fn("put_HZ", hz), P()]
    for m in (0, 1):
        b += [mark(f"mode {m}"), fn("put_Mode", m), P(1.2)]
    for bl in (0, 8, 16, 31):
        b += [mark(f"blacklevel {bl}"), opt(OPT_BLACKLEVEL, bl), P()]
    for o, vals, name in ((OPT_CG, (1, 0), "cg"), (OPT_LOW_NOISE, (1, 0), "lownoise"),
                          (OPT_HIGH_FULLWELL, (1, 0), "highfullwell"),
                          (OPT_FRAMERATE, (5, 0), "framerate"),
                          (OPT_PRECISE_FRAMERATE, (50, 0), "precise_framerate"),
                          (OPT_BANDWIDTH, (50, 100), "bandwidth"),
                          (OPT_DEFECT_PIXEL, (0, 1), "defectpixel"),
                          (OPT_ZERO_OFFSET, (1, 0), "zerooffset")):
        for v in vals:
            b += [mark(f"{name} {v}"), opt(o, v), P()]
    for flip in ("HFlip", "VFlip"):
        b += [mark(f"{flip} 1"), fn(f"put_{flip}", 1), P(), fn(f"put_{flip}", 0), P(0.3)]
    for o in (OPT_UPTIME, OPT_LINE_TIME, OPT_MAX_PRECISE_FRAMERATE):
        b += [{"getoption": o}, P(0.3)]
    b += [mark("roi 1000x800 @ 200,100"), fn("put_Roi", 200, 100, 1000, 800), P(1.5),
          {"save": "roi_raw8"}, mark("roi off"), fn("put_Roi", 0, 0, 0, 0), P(1.5)]
    b += [mark("autoexpo on"), fn("put_AutoExpoEnable", 1), P(3),
          mark("aetarget 60"), fn("put_AutoExpoTarget", 60), P(3),
          mark("autoexpo off"), fn("put_AutoExpoEnable", 0), P()]
    return session("raw8", 2, b)


def test_patterns():
    b = []
    for tp in (3, 5, 7, 9, 0):
        b += [mark(f"testpattern {tp}"), opt(OPT_TESTPATTERN, tp), P(1.2), {"save": f"tp{tp}"}]
    steps = session("raw8", 2, b)
    steps += session("raw16", 2, [mark("testpattern 9"), opt(OPT_TESTPATTERN, 9), P(1.2),
                                  {"save": "tp9"}, opt(OPT_TESTPATTERN, 0), P(0.5)])
    return steps


def isp_controls():
    b = [mark("baseline"), P(1), {"save": "isp_base"}]
    for t, n in ((2000, 1000), (15000, 1000), (6503, 200), (6503, 2500), (6503, 1000)):
        b += [mark(f"temptint {t} {n}"), fn("put_TempTint", t, n), P()]
    for g in ((-127, 0, 127), (50, -50, 0), (0, 0, 0)):
        b += [mark(f"wbgain {g}"), fn("put_WhiteBalanceGain", {"i32": list(g)}), P()]
    for name, vals, d in (("Hue", (-180, 90), 0), ("Saturation", (0, 255), 128),
                          ("Brightness", (-255, 255), 0), ("Contrast", (-255, 255), 0),
                          ("Gamma", (20, 180), 100)):
        for v in vals + (d,):
            b += [mark(f"{name.lower()} {v}"), fn(f"put_{name}", v), P()]
    b += [mark("levelrange"), fn("put_LevelRange", {"u16": [20, 20, 20, 20]},
                                 {"u16": [200, 200, 200, 200]}), P(),
          fn("put_LevelRange", {"u16": [0, 0, 0, 0]}, {"u16": [255, 255, 255, 255]}), P()]
    b += [mark("chrome 1"), fn("put_Chrome", 1), P(), fn("put_Chrome", 0), P()]
    b += [mark("negative 1"), fn("put_Negative", 1), P(), fn("put_Negative", 0), P()]
    for o, vals, name in ((OPT_COLORMATIX, (0, 1), "colormatrix"), (OPT_WBGAIN, (0, 1), "wbgain"),
                          (OPT_LINEAR, (0, 1), "linear"), (OPT_CURVE, (0, 1), "curve"),
                          (OPT_ISP, (-1, 1, 0), "isp"), (OPT_DENOISE, (50, 0), "denoise"),
                          (OPT_SHARPENING, (100 | (2 << 16), 0), "sharpening")):
        for v in vals:
            b += [mark(f"{name} {v}"), opt(o, v), P()]
    b += [mark("awb once"), {"fn": "AwbOnce", "args": [0, 0]}, P(3)]
    return session("rgb24", 2, b)


def still_trigger():
    b = [mark("still 0 (full res) while previewing size2"), fn("Snap", 0), P(4),
         mark("still 1"), fn("Snap", 1), P(3)]
    b += [mark("trigger mode 1"), opt(OPT_TRIGGER, 1), P(1.5),
          mark("trigger 1"), fn("Trigger", 1), P(1.5),
          mark("trigger 3"), fn("Trigger", 3), P(2),
          mark("trigger mode 0"), opt(OPT_TRIGGER, 0), P(1.5)]
    return session("raw8", 2, b)


if __name__ == "__main__":
    for name, f in (("modes", modes), ("sensor_controls", sensor_controls),
                    ("test_patterns", test_patterns), ("isp_controls", isp_controls),
                    ("still_trigger", still_trigger)):
        with open(os.path.join(HERE, f"{name}.json"), "w") as fh:
            json.dump(f(), fh, indent=1)
        print("wrote", name)
