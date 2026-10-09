"""Drive the vendor amcam SDK through a scripted sequence of calls while USBPcap
records, so USB traffic can be correlated with SDK calls.

Every step is printed with a high-resolution wall-clock timestamp (also written to
a .jsonl log next to the capture) so pcap timestamps can be matched to calls.

Usage: python re/sdk_probe.py <scenario> [logfile]
"""

import ctypes as C
import json
import sys
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

LOG = None


def log(step, **kw):
    rec = {"t": time.time(), "step": step, **kw}
    print(f"{rec['t']:.6f} {step} {kw if kw else ''}", flush=True)
    if LOG:
        LOG.write(json.dumps(rec) + "\n")
        LOG.flush()


def call(name, *args, restype=HRESULT):
    fn = getattr(dll, name)
    fn.restype = restype
    log("call>" + name, args=[a if isinstance(a, (int, float, str)) else None for a in args])
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
    get_int(h, "Amcam_get_Temperature", C.c_short)
    lo, hi, df = C.c_uint(), C.c_uint(), C.c_uint()
    call("Amcam_get_ExpTimeRange", h, C.byref(lo), C.byref(hi), C.byref(df))
    log("value", name="ExpTimeRange", value=[lo.value, hi.value, df.value])
    glo, ghi, gdf = C.c_ushort(), C.c_ushort(), C.c_ushort()
    call("Amcam_get_ExpoAGainRange", h, C.byref(glo), C.byref(ghi), C.byref(gdf))
    log("value", name="ExpoAGainRange", value=[glo.value, ghi.value, gdf.value])


frames = []


def stream(h, model, seconds, raw=False, size_idx=None):
    if raw:
        call("Amcam_put_Option", h, 0x04, 1)  # AMCAM_OPTION_RAW
    if size_idx is not None:
        call("Amcam_put_eSize", h, size_idx)
    w, hgt = C.c_int(), C.c_int()
    call("Amcam_get_Size", h, C.byref(w), C.byref(hgt))
    log("value", name="Size", value=[w.value, hgt.value])
    buf = (C.c_ubyte * (w.value * hgt.value * 4))()
    events = []

    @CB
    def cb(ev, ctx):
        events.append((time.time(), ev))

    call("Amcam_StartPullModeWithCallback", h, cb, None)
    end = time.time() + seconds
    while time.time() < end:
        while events:
            t, ev = events.pop(0)
            if ev == 0x0004:  # AMCAM_EVENT_IMAGE
                fi = FrameInfoV2()
                hr = dll.Amcam_PullImageV2(h, buf, 8 if raw else 24, C.byref(fi))
                log("frame", hr=hr, w=fi.width, h=fi.height, seq=fi.seq, ts=fi.timestamp, flag=fi.flag)
                if len(frames) < 3:
                    frames.append(bytes(buf[: fi.width * fi.height * (1 if raw else 3)]))
            else:
                log("event", ev=ev)
        time.sleep(0.005)
    call("Amcam_Stop", h)
    return cb  # keep alive


def scenario_open_close():
    h, m = open_cam()
    info(h)
    pause(1, "idle after info")
    call("Amcam_Close", h)


def scenario_stream_controls():
    h, m = open_cam()
    info(h)
    call("Amcam_put_AutoExpoEnable", h, 0)
    call("Amcam_put_ExpoTime", h, 10000)
    call("Amcam_put_ExpoAGain", h, 100)
    keep = stream(h, m, 3, raw=True, size_idx=2)
    pause(0.5, "between")
    # Controls while streaming: each separated by a pause so traffic is attributable.
    keep2_events = []
    keep2 = CB(lambda e, c: keep2_events.append(e))  # must outlive streaming
    call("Amcam_StartPullModeWithCallback", h, keep2, None)
    pause(1, "streaming baseline")
    for us in (5000, 20000, 50000):
        call("Amcam_put_ExpoTime", h, us)
        pause(1, f"expo {us}")
    for g in (100, 200, 400):
        call("Amcam_put_ExpoAGain", h, g)
        pause(1, f"gain {g}")
    call("Amcam_put_HFlip", h, 1); pause(1, "hflip")
    call("Amcam_put_VFlip", h, 1); pause(1, "vflip")
    call("Amcam_put_Speed", h, 0); pause(1, "speed 0")
    call("Amcam_put_Speed", h, 2); pause(1, "speed 2")
    call("Amcam_Stop", h)
    call("Amcam_Close", h)
    with open("captures/frames_raw_size2.bin", "wb") as f:
        for fr in frames:
            f.write(fr)


if __name__ == "__main__":
    name = sys.argv[1]
    if len(sys.argv) > 2:
        LOG = open(sys.argv[2], "w")
    log("sdk", version=dll.Amcam_Version())
    globals()["scenario_" + name]()
