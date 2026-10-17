"""Tests against the simulated camera (no hardware needed)."""

import io
import threading

import numpy as np
import pytest

import loupecam

SMALLEST = 2  # 1228×922


@pytest.fixture
def cam():
    with loupecam.open_simulated() as c:
        yield c


def test_model_and_info(cam):
    assert cam.simulated
    assert cam.model.name == "MU1803-HS"
    assert cam.model.resolutions[0] == (4912, 3684)
    assert cam.model.bayer_pattern == "RGGB"
    assert cam.info.serial == "SIMULATED"
    assert isinstance(loupecam.__version__, str)


def test_raw8(cam):
    stream = cam.start(SMALLEST, "raw8")
    frame = stream.read(timeout=5)
    assert (frame.width, frame.height, frame.bits) == (1228, 922, 8)
    raw = frame.raw
    assert raw.dtype == np.uint8 and raw.shape == (922, 1228)
    assert raw.max() > raw.min()
    assert np.array_equal(np.asarray(frame), raw)
    g = cam.geometry
    assert (g.width, g.height, g.bits) == (1228, 922, 8)


def test_raw12(cam):
    frame = cam.start(SMALLEST, "raw12").read(timeout=5)
    raw = frame.raw
    assert raw.dtype == np.uint16 and raw.shape == (922, 1228)
    assert frame.bits == 12 and raw.max() < 4096


def test_develop(cam):
    frame = cam.start(SMALLEST, "raw12").read(timeout=5)
    rgb = frame.develop()
    assert rgb.dtype == np.uint8 and rgb.shape == (922, 1228, 3)
    half = frame.develop(demosaic="superpixel")
    assert half.shape == (461, 614, 3)
    deep = frame.develop(depth=16, demosaic="mhc", white_balance="auto")
    assert deep.dtype == np.uint16
    mono = frame.develop(monochrome=True)
    assert mono.shape == (922, 1228)
    turned = frame.develop(rotation=90)
    assert turned.shape == (1228, 922, 3)
    # Defaults are the model's daylight balance and colour matrix, which keep the
    # simulator's grey specimen roughly neutral.
    r, g, b = rgb.reshape(-1, 3).mean(axis=0)
    assert abs(r - g) < 0.15 * g and abs(b - g) < 0.15 * g


def test_develop_rejects_bad_options(cam):
    frame = cam.start(SMALLEST).read(timeout=5)
    with pytest.raises(TypeError):
        frame.develop(colour="red")
    with pytest.raises(ValueError):
        frame.develop(demosaic="magic")
    with pytest.raises(ValueError):
        frame.develop(rotation=45)


def test_encode_and_save(cam, tmp_path):
    frame = cam.start(SMALLEST, "raw12").read(timeout=5)
    assert frame.encode("png").startswith(b"\x89PNG")
    assert frame.encode("jpeg", quality=80).startswith(b"\xff\xd8")
    assert frame.encode("tiff", raw=True)[:2] in (b"II", b"MM")
    with pytest.raises(ValueError):
        frame.encode("jpeg", raw=True)
    path = tmp_path / "shot.tif"
    frame.save(path, depth=16)
    assert path.stat().st_size > 1228 * 922 * 6
    with pytest.raises(ValueError):
        frame.save(tmp_path / "shot.bmp")


def test_white_balance_and_stats(cam):
    frame = cam.start(SMALLEST).read(timeout=5)
    r, g, b = frame.white_balance()
    assert g == 1.0 and r > 1 and b > 1  # the simulator's red and blue respond weakly
    s = frame.stats(region=(0, 0, 200, 200))
    assert len(s.histogram) == 256 and 0 < s.luma < 1


def test_controls(cam):
    cam.start(SMALLEST, exposure_us=10_000, gain=2.0)
    assert cam.exposure_us == 10_000
    assert cam.gain == pytest.approx(2.0, rel=0.1)
    assert cam.set_exposure(5_000) == pytest.approx(5_000, rel=0.05)
    cam.set_gain(1.0)
    g = cam.set_roi((100, 100, 400, 300))
    assert (g.width, g.height) == (400, 300)
    with pytest.raises(ValueError):
        cam.set_gain(1000.0)


def test_iteration_and_restart(cam):
    stream = cam.start(SMALLEST)
    frames = []
    for f in stream:
        frames.append(f.sequence)
        if len(frames) == 3:
            break
    assert frames == sorted(frames)
    assert stream.stats.frames >= 3
    # Restarting ends the old stream.
    cam.start(SMALLEST)
    with pytest.raises(loupecam.StreamStopped):
        while True:
            stream.read(timeout=5)
    assert list(stream) == []


def test_timeout(cam):
    stream = cam.start(SMALLEST)
    cam.stop()
    with pytest.raises((TimeoutError, loupecam.StreamStopped)):
        stream.read(timeout=0.2)


def test_read_releases_the_gil(cam):
    """A reader thread blocked in read() mustn't stop other threads running Python."""
    stream = cam.start(SMALLEST)
    got = []
    t = threading.Thread(target=lambda: got.append(stream.read(timeout=5)))
    t.start()
    counter = sum(range(100_000))  # runs while the reader waits
    t.join()
    assert counter and got


def test_close(cam):
    cam.close()
    assert cam.closed
    cam.close()
    with pytest.raises(loupecam.LoupecamError):
        cam.start()
    assert "closed" in repr(cam)


def test_open_without_camera():
    try:
        cameras = loupecam.list_cameras()
    except loupecam.LoupecamError:
        cameras = []  # no USB subsystem at all (WSL, containers)
    if cameras:
        pytest.skip("a real camera is connected")
    with pytest.raises(loupecam.LoupecamError):
        loupecam.open()


def test_png_round_trip_if_pillow(cam):
    Image = pytest.importorskip("PIL.Image")
    frame = cam.start(SMALLEST).read(timeout=5)
    img = Image.open(io.BytesIO(frame.encode("png")))
    assert img.size == (1228, 922)
    assert np.array_equal(np.asarray(img), frame.develop())
