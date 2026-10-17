"""Driver for AmScope / ToupTek USB3 microscope cameras.

It talks to the camera directly over USB: no vendor SDK and no kernel driver.

    import loupecam

    with loupecam.open() as cam:              # or loupecam.open_simulated()
        stream = cam.start(size=0, mode="raw12", exposure_us=30_000)
        frame = stream.read(timeout=2)
        rgb = frame.develop()                 # (height, width, 3) uint8 array
        frame.save("photo.png", white_balance="auto")
"""

from ._native import (
    Camera,
    CameraDevice,
    CameraInfo,
    Frame,
    FrameStats,
    Geometry,
    LoupecamError,
    Model,
    Stream,
    StreamStats,
    StreamStopped,
    __version__,
    list_cameras,
    open,
    open_simulated,
)

__all__ = [
    "Camera",
    "CameraDevice",
    "CameraInfo",
    "Frame",
    "FrameStats",
    "Geometry",
    "LoupecamError",
    "Model",
    "Stream",
    "StreamStats",
    "StreamStopped",
    "__version__",
    "list_cameras",
    "open",
    "open_simulated",
]
