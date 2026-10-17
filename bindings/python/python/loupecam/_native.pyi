"""Type stubs for the native extension (re-exported by `loupecam`)."""

import os
from collections.abc import Iterator, Sequence
from types import TracebackType
from typing import Literal, Optional, TypedDict, Union, final

import numpy as np
import numpy.typing as npt

__version__: str

class LoupecamError(Exception):
    """An error from the camera or the driver."""

class StreamStopped(LoupecamError):
    """The stream ended: the camera was stopped, restarted or closed."""

PixelMode = Literal["raw8", "raw12", "processed8"]
Demosaic = Literal["superpixel", "bilinear", "mhc"]
ImageFormat = Literal["png", "tiff", "tif", "jpeg", "jpg"]
Region = tuple[int, int, int, int]
"""`(x, y, width, height)` in pixels."""

class DevelopOptions(TypedDict, total=False):
    """Keyword arguments of `Frame.develop()`, `Frame.encode()` and `Frame.save()`.

    Every option defaults to the camera model's own settings: its colour matrix and
    daylight white balance, bilinear demosaicing, the sRGB curve, upright.
    """

    demosaic: Demosaic
    """`"superpixel"` (half size, fastest), `"bilinear"` (default) or `"mhc"`
    (Malvar-He-Cutler: sharpest, about twice the cost of bilinear)."""
    white_balance: Union[Literal["daylight", "auto", "none"], tuple[float, float, float], None]
    """`"daylight"` (the model's default), `"auto"` (grey world over the whole frame),
    `"none"` (unity), or `(r, g, b)` gains, e.g. from `Frame.white_balance(region)`."""
    color_matrix: Union[bool, Sequence[Sequence[float]], None]
    """`True`/`None` for the model's matrix, `False` for none, or a 3×3 row-major
    matrix applied to linear camera RGB."""
    tone: Union[Literal["srgb", "linear"], float]
    """Transfer curve: `"srgb"` (default), `"linear"`, or a gamma value."""
    black_point: float
    """Input level (0-1) mapped to black."""
    white_point: float
    """Input level (0-1) mapped to white."""
    brightness: float
    """-1 to 1."""
    contrast: float
    """-1 to 1."""
    black_level: int
    """Sensor black level to subtract, in the frame's sample units."""
    monochrome: bool
    """A single luminance channel."""
    negative: bool
    flip_horizontal: bool
    flip_vertical: bool
    """Relative to upright (the MU1803's bottom-up readout is already corrected)."""
    rotation: Literal[0, 90, 180, 270]
    """Clockwise, applied after flips."""
    depth: Literal[8, 16]
    """Output bits per channel."""

@final
class Model:
    """A camera model: its sensor, sizes and colour filter."""

    @property
    def name(self) -> str: ...
    @property
    def vendor_id(self) -> int: ...
    @property
    def product_id(self) -> int: ...
    @property
    def resolutions(self) -> list[tuple[int, int]]:
        """Streamable `(width, height)` sizes, largest first. The index is the `size`
        passed to `Camera.start()`."""
    @property
    def pixel_size_um(self) -> float: ...
    @property
    def bayer_pattern(self) -> Optional[str]:
        """The mosaic's top-left 2×2 cell, e.g. `"RGGB"`; `None` for monochrome."""
    @property
    def max_bit_depth(self) -> int:
        """Significant bits per sample in the `raw12` mode."""

@final
class CameraDevice:
    """A connected camera that isn't open yet."""

    @property
    def model(self) -> Model: ...
    @property
    def id(self) -> str:
        """Platform-specific identifier, stable while the camera stays plugged in."""
    @property
    def bus(self) -> str: ...
    @property
    def port_chain(self) -> list[int]: ...
    def open(self) -> Camera: ...

@final
class CameraInfo:
    """Identity and versions read from an open camera."""

    @property
    def model(self) -> Model: ...
    @property
    def serial(self) -> str: ...
    @property
    def firmware_version(self) -> str: ...
    @property
    def hardware_version(self) -> str: ...
    @property
    def fpga_version(self) -> tuple[int, int]: ...
    @property
    def sensor_id(self) -> int: ...
    @property
    def production_date(self) -> Optional[tuple[int, int, int]]:
        """`(year, month, day)`, when the camera records it."""

@final
class Geometry:
    """Geometry and timing of a running stream."""

    width: int
    height: int
    bits: int
    """Significant bits per sample (8 or 12)."""
    exposure_us: float
    """The exposure actually programmed."""
    max_fps: float
    """The highest frame rate this geometry allows at the current exposure."""

@final
class StreamStats:
    """Counters for a running stream."""

    frames: int
    dropped: int
    """Frames dropped because the reader didn't keep up (the queue holds 3)."""
    discarded: int
    bytes: int
    transfer_errors: int
    fps: float
    """Frame rate over the last second."""
    sequence_gaps: int
    """Frames missing according to the camera's sequence counter."""

@final
class FrameStats:
    """Statistics of a frame's raw mosaic."""

    mean: tuple[float, float, float]
    """Mean linear level of R, G and B, 0 to 1."""
    luma: float
    """Mean luma (Rec. 709) after white balance, 0 to 1."""
    clipped: float
    """Fraction of 2×2 cells with a channel at or near full scale."""
    histogram: list[int]
    """256-bin histogram of per-cell linear luma."""

@final
class Frame:
    """One frame: the raw Bayer mosaic as the sensor delivered it."""

    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    @property
    def bits(self) -> int:
        """Significant bits per sample: 8, or 12 in the `raw12` mode."""
    @property
    def sequence(self) -> int:
        """The camera's frame counter."""
    @property
    def bayer_pattern(self) -> Optional[str]: ...
    @property
    def model(self) -> Model: ...
    @property
    def raw(self) -> Union[npt.NDArray[np.uint8], npt.NDArray[np.uint16]]:
        """The mosaic as a `(height, width)` array: `uint8`, or `uint16` holding `bits`
        significant bits. Rows are as the sensor sends them (bottom-up on the MU1803;
        `develop()` turns them upright). Each access makes a new copy."""
    def __array__(self, dtype: Optional[npt.DTypeLike] = None, copy: Optional[bool] = None) -> np.ndarray:
        """`numpy.asarray(frame)` is `frame.raw`."""
    def develop(
        self,
        *,
        demosaic: Demosaic = ...,
        white_balance: Union[Literal["daylight", "auto", "none"], tuple[float, float, float], None] = ...,
        color_matrix: Union[bool, Sequence[Sequence[float]], None] = ...,
        tone: Union[Literal["srgb", "linear"], float] = ...,
        black_point: float = ...,
        white_point: float = ...,
        brightness: float = ...,
        contrast: float = ...,
        black_level: int = ...,
        monochrome: bool = ...,
        negative: bool = ...,
        flip_horizontal: bool = ...,
        flip_vertical: bool = ...,
        rotation: Literal[0, 90, 180, 270] = ...,
        depth: Literal[8, 16] = ...,
    ) -> Union[npt.NDArray[np.uint8], npt.NDArray[np.uint16]]:
        """Develop into an image: `(height, width, 3)` RGB, or `(height, width)` with
        `monochrome=True`; `uint8`, or `uint16` with `depth=16`. See `DevelopOptions`."""
    def encode(self, format: ImageFormat = "png", *, quality: int = 90, raw: bool = False, **options: object) -> bytes:
        """Develop (with `DevelopOptions`) and encode. JPEG `quality` is 1-100. With
        `raw=True` (TIFF only), the undeveloped mosaic scaled to 16 bits."""
    def save(self, path: Union[str, os.PathLike[str]], *, quality: int = 90, raw: bool = False, **options: object) -> None:
        """Like `encode()`, writing a file; the format comes from the extension
        (`.png`, `.tif`/`.tiff`, `.jpg`/`.jpeg`)."""
    def white_balance(self, region: Optional[Region] = None) -> tuple[float, float, float]:
        """Grey-world `(r, g, b)` gains. Over a neutral `region`, this is "click to
        white balance"."""
    def stats(self, region: Optional[Region] = None, white_balance: Optional[tuple[float, float, float]] = None) -> FrameStats:
        """Raw statistics. `white_balance` weights the luma estimate (default: the
        model's daylight balance)."""

@final
class Stream:
    """Frames from a running camera. Iterating yields frames until the stream ends."""

    def read(self, timeout: Optional[float] = None) -> Frame:
        """The next frame, waiting up to `timeout` seconds (forever if `None`). Raises
        `TimeoutError`, or `StreamStopped` once the stream has ended."""
    def latest(self, timeout: Optional[float] = 1.0) -> Frame:
        """The newest frame, discarding older queued ones (for live views)."""
    def try_read(self) -> Optional[Frame]:
        """A frame if one is ready, else `None`."""
    @property
    def stats(self) -> StreamStats: ...
    def __iter__(self) -> Iterator[Frame]: ...
    def __next__(self) -> Frame: ...

@final
class Camera:
    """An open camera. Closing it (or leaving a `with` block) releases the device."""

    @property
    def info(self) -> CameraInfo: ...
    @property
    def model(self) -> Model: ...
    @property
    def simulated(self) -> bool: ...
    @property
    def streaming(self) -> bool: ...
    @property
    def closed(self) -> bool: ...
    @property
    def geometry(self) -> Optional[Geometry]:
        """The running stream's geometry, or `None` when not streaming."""
    @property
    def stats(self) -> Optional[StreamStats]: ...
    @property
    def exposure_us(self) -> Optional[int]:
        """The requested exposure, while streaming."""
    @property
    def gain(self) -> Optional[float]:
        """The gain multiplier, while streaming."""
    @staticmethod
    def max_gain() -> float:
        """The largest multiplier `set_gain()` accepts."""
    def start(
        self,
        size: int = 0,
        mode: PixelMode = "raw8",
        *,
        exposure_us: int = 20_000,
        gain: float = 1.0,
        speed: int = 3,
        binning: bool = False,
        roi: Optional[Region] = None,
    ) -> Stream:
        """Start streaming. `size` indexes `model.resolutions` (0 = largest). `raw12`
        gives 12-bit samples; `processed8` applies the camera's own ISP. `speed` is the
        readout speed, 0-3. Starting again reconfigures, and earlier streams end."""
    def stop(self) -> None:
        """Stop streaming (the camera stays open)."""
    def close(self) -> None:
        """Stop streaming and release the camera. Closing twice is harmless."""
    def set_exposure(self, exposure_us: int) -> float:
        """Returns the exposure actually programmed, in µs."""
    def set_gain(self, gain: float) -> None:
        """A multiplier: 1.0 is unity."""
    def set_speed(self, speed: int) -> Geometry: ...
    def set_binning(self, binning: bool) -> None: ...
    def set_roi(self, roi: Optional[Region]) -> Geometry:
        """Stream only a region of the current size, or the whole frame with `None`."""
    def __enter__(self) -> Camera: ...
    def __exit__(
        self,
        exc_type: Optional[type[BaseException]],
        exc: Optional[BaseException],
        tb: Optional[TracebackType],
    ) -> None: ...

def list_cameras() -> list[CameraDevice]:
    """Connected cameras of supported models."""

def open(device: Union[CameraDevice, int, str, None] = None) -> Camera:
    """Open a camera: the first one found (`None`), a `CameraDevice`, an index into
    `list_cameras()`, or a device `id`."""

def open_simulated() -> Camera:
    """A simulated MU1803-HS: synthetic frames, no hardware needed."""
