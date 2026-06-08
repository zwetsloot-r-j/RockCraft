"""Frame loading for the extractor CLI.

Input is a **local path only** — no downloading (that is M6-D's pluggable hook).
Two forms are accepted so tests stay hermetic (no video codec required):

* a video file (anything ``cv2.VideoCapture`` can open), or
* a directory of numbered frames (``*.png`` / ``*.jpg`` sorted by name), paired
  with an explicit ``--fps``.
"""

from __future__ import annotations

import os
from typing import Iterator, Optional

import cv2
import numpy as np

_FRAME_EXTS = (".png", ".jpg", ".jpeg", ".bmp")


def is_frame_dir(path: str) -> bool:
    return os.path.isdir(path)


def load_frames(path: str, fps_override: Optional[float] = None) -> tuple[list[np.ndarray], float]:
    """Load all frames and the source fps from a file or a directory."""
    if is_frame_dir(path):
        names = sorted(
            n for n in os.listdir(path) if n.lower().endswith(_FRAME_EXTS)
        )
        frames = [cv2.imread(os.path.join(path, n), cv2.IMREAD_COLOR) for n in names]
        frames = [f for f in frames if f is not None]
        if fps_override is None:
            raise ValueError("--fps is required when --in is a directory of frames")
        return frames, float(fps_override)

    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise FileNotFoundError(f"could not open video: {path}")
    fps = fps_override or cap.get(cv2.CAP_PROP_FPS) or 30.0
    frames: list[np.ndarray] = []
    for f in _iter_capture(cap):
        frames.append(f)
    cap.release()
    return frames, float(fps)


def _iter_capture(cap: "cv2.VideoCapture") -> Iterator[np.ndarray]:
    while True:
        ok, frame = cap.read()
        if not ok:
            break
        yield frame
