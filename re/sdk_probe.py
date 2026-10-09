"""Drive the vendor amcam SDK through a scripted sequence of calls while USBPcap
records, so USB traffic can be correlated with SDK calls (see correlate.py).

Every step is logged with a wall-clock timestamp to a .jsonl file next to the capture.
pcap timestamps come from the same clock, so traffic can be attributed to steps.

Usage:
  python re/sdk_probe.py <scenario> [logfile]       built-in scenario (open_close)
  python re/sdk_probe.py <steps.json> [logfile]     data-driven sweep (see re/sweeps/)

Step syntax (a JSON list, executed in order):
  {"open": true}                     enumerate + open the first camera
  {"close": true}
  {"start": true} / {"stop": true}   pull-mode streaming (a background thread pulls frames)
  {"fn": "put_ExpoTime", "args": [5000]}   any Amcam_* function; the handle is implicit.
        Args: int, or {"u16": [...]} / {"i32": [...]} for array pointers.
  {"get": "get_ExpoTime"}            int out-parameter getter
  {"option": 0x28, "value": 1}       put_Option
  {"getoption": 0x28}                get_Option
  {"pause": 1.0}
  {"save": "name"}                   write the most recent frame to captures/frames/<name>.bin
  {"pullbits": 8|16|24|48}           bits for PullImageV2 (8/16 = raw, 24 = RGB24)
"""

import ctypes as C
import json
import os
import sys
import threading
import time

DLL = r"C:\Program Files\AmScope\AmScope\x64\amcam.dll"
dll = C.WinDLL(DLL)

HRESULT = C.c_long
H = C.c_void_p


class Res(C.Structure):
    _fields_ = [("w", C.c_uint), ("h", C.c_uint)]


class Model(C.Structure):
    _fields_ = [
        ("name", C.c_wchar_p), ("flag", C.c_ulonglong), ("maxspeed", C.c_uint),
        ("preview", C.c_uint), ("still", C.c_uint), ("maxfanspeed", C.c_uint),
        ("ioctrol", C.c_uint), ("xpix", C.c_float), ("ypix", C.c_float), ("res", Res * 16),
    ]


class Dev(C.Structure):
    _fields_ = [("disp", C.c_wchar * 64), ("id", C.c_wchar * 64), ("model", C.POINTER(Model))]


class FrameInfoV2(C.Structure):
    _fields_ = [("width", C.c_uint), ("height", C.c_uint), ("flag", C.c_uint),
                ("seq", C.c_uint), ("timestamp", C.c_ulonglong)]


dll.Amcam_Version.restype = C.c_wchar_p
dll.Amcam_Open.restype = H
dll.Amcam_Open.argtypes = [C.c_wchar_p]
CB = C.WINFUNCTYPE(None, C.c_uint, C.c_void_p)
EVENT_IMAGE, EVENT_STILLIMAGE = 0x0004, 0x0005

LOG = None


def log(step, **kw):
    rec = {"t": time.time(), "step": step, **kw}
    print(f"{rec['t']:.6f} {step} {kw if kw else ''}", flush=True)
    if LOG:
        LOG.write(json.dumps(rec) + "\n")
        LOG.flush()


def call(name, *args, restype=HRESULT, quiet_args=None):
    fn = getattr(dll, name)
    fn.restype = restype
    shown = quiet_args if quiet_args is not None else [
        a if isinstance(a, (int, float, str)) else None for a in args]
    log("call>" + name, args=shown)
    r = fn(*args)
    log("call<" + name, ret=r if isinstance(r, (int, float, str)) or r is None else str(r))
    return r


def get_str(h, name, n=64):
    buf = C.create_string_buffer(n)
    hr = call(name, h, buf)
    v = buf.value.decode(errors="replace")
    log("value", name=name, hr=hr, value=v)
    return v


def get_int(h, name, ctype=C.c_int):
    v = ctype()
    hr = call(name, h, C.byref(v))
    log("value", name=name, hr=hr, value=v.value)
    return v.value


def pause(s, why=""):
    log("sleep", seconds=s, why=why)
    time.sleep(s)


def open_cam():
    arr = (Dev * 128)()
    n = dll.Amcam_EnumV2(arr)
    if n < 1:
        sys.exit("no camera")
    d = arr[0]
    log("enum", n=n, disp=d.disp, id=d.id, model=d.model.contents.name)
    h = call("Amcam_Open", d.id, restype=H)
    if not h:
        sys.exit("open failed")
    return H(h), d.model.contents


def info(h):
    for n in ("Amcam_get_SerialNumber", "Amcam_get_FwVersion", "Amcam_get_HwVersion",
              "Amcam_get_ProductionDate", "Amcam_get_FpgaVersion"):
        get_str(h, n)
    get_int(h, "Amcam_get_Revision", C.c_ushort)
    get_int(h, "Amcam_get_MaxSpeed")
    get_int(h, "Amcam_get_MaxBitDepth")
    lo, hi, df = C.c_uint(), C.c_uint(), C.c_uint()
    call("Amcam_get_ExpTimeRange", h, C.byref(lo), C.byref(hi), C.byref(df))
    log("value", name="ExpTimeRange", value=[lo.value, hi.value, df.value])
    glo, ghi, gdf = C.c_ushort(), C.c_ushort(), C.c_ushort()
    call("Amcam_get_ExpoAGainRange", h, C.byref(glo), C.byref(ghi), C.byref(gdf))
    log("value", name="ExpoAGainRange", value=[glo.value, ghi.value, gdf.value])


class Streamer:
    """Pull-mode streaming with a background thread that pulls every frame."""

    def __init__(self, h):
        self.h = h
        self.bits = 8
        self.last = None
        self.count = 0
        self.ev = threading.Event()
        self.pending = []
        self.lock = threading.Lock()
        self.running = False
        self.cb = CB(self._cb)  # must outlive streaming

    def _cb(self, ev, ctx):
        with self.lock:
            self.pending.append(ev)
        self.ev.set()

    # Largest possible frame: full sensor at 48 bits/pixel.
    BUF = (C.c_ubyte * (4912 * 3684 * 6))()

    def _pull(self, still):
        fi = FrameInfoV2()
        fn = dll.Amcam_PullStillImageV2 if still else dll.Amcam_PullImageV2
        hr = fn(self.h, self.BUF, self.bits, C.byref(fi))
        n = fi.width * fi.height * max(1, self.bits // 8)
        self.count += 1
        self.last = (fi.width, fi.height, self.bits, C.string_at(self.BUF, n))
        log("still" if still else "frame", hr=hr, w=fi.width, h=fi.height, seq=fi.seq,
            ts=fi.timestamp, flag=fi.flag)

    def _loop(self):
        while self.running:
            if not self.ev.wait(0.1):
                continue
            self.ev.clear()
            with self.lock:
                evs, self.pending = self.pending, []
            for ev in evs:
                if ev in (EVENT_IMAGE, EVENT_STILLIMAGE):
                    try:
                        self._pull(ev == EVENT_STILLIMAGE)
                    except OSError as e:
                        log("pull-error", err=str(e))
                else:
                    log("event", ev=ev)

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._loop, daemon=True)
        self.thread.start()
        call("Amcam_StartPullModeWithCallback", self.h, self.cb, None)

    def stop(self):
        call("Amcam_Stop", self.h)
        self.running = False
        self.thread.join()


def conv_arg(a):
    if isinstance(a, dict):
        if "u16" in a:
            return (C.c_ushort * len(a["u16"]))(*a["u16"])
        if "i32" in a:
            return (C.c_int * len(a["i32"]))(*a["i32"])
    if isinstance(a, float):
        return C.c_double(a)
    return a


def run_steps(steps):
    h = st = None
    os.makedirs("captures/frames", exist_ok=True)
    for s in steps:
        if "open" in s:
            h, _ = open_cam()
            st = Streamer(h)
        elif "close" in s:
            call("Amcam_Close", h)
            h = None
        elif "start" in s:
            st.start()
        elif "stop" in s:
            st.stop()
        elif "pullbits" in s:
            st.bits = s["pullbits"]
            log("pullbits", bits=st.bits)
        elif "fn" in s:
            args = [conv_arg(a) for a in s.get("args", [])]
            call("Amcam_" + s["fn"], h, *args, quiet_args=s.get("args", []))
        elif "get" in s:
            get_int(h, "Amcam_" + s["get"])
        elif "option" in s:
            call("Amcam_put_Option", h, s["option"], s["value"],
                 quiet_args=[s["option"], s["value"]])
        elif "getoption" in s:
            v = C.c_int()
            hr = call("Amcam_get_Option", h, s["getoption"], C.byref(v))
            log("value", name=f"option 0x{s['getoption']:02x}", hr=hr, value=v.value)
        elif "info" in s:
            info(h)
        elif "pause" in s:
            pause(s["pause"], s.get("why", ""))
        elif "save" in s:
            if st and st.last:
                w, hh, bits, data = st.last
                path = f"captures/frames/{s['save']}_{w}x{hh}_{bits}b.bin"
                open(path, "wb").write(data)
                log("saved", path=path)
            else:
                log("save-skipped", why="no frame yet")
        elif "mark" in s:
            log("mark", label=s["mark"])
        else:
            raise ValueError(f"unknown step {s}")


def scenario_open_close():
    h, m = open_cam()
    info(h)
    pause(1, "idle after info")
    call("Amcam_Close", h)


if __name__ == "__main__":
    name = sys.argv[1]
    if len(sys.argv) > 2:
        LOG = open(sys.argv[2], "w")
    log("sdk", version=dll.Amcam_Version())
    if name.endswith(".json"):
        run_steps(json.load(open(name)))
    else:
        globals()["scenario_" + name]()
