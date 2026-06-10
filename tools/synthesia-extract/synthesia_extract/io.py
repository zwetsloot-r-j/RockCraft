"""Frame loading for the extractor CLI.

Input is a **local path only** — no downloading (that is M6-D's pluggable hook).
Two forms are accepted so tests stay hermetic (no video codec required):

* a video file (anything ``cv2.VideoCapture`` can open), or
* a directory of numbered frames (``*.png`` / ``*.jpg`` sorted by name), paired
  with an explicit ``--fps``.
"""

from __future__ import annotations

import os
import wave
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

    # Long videos do not fit in RAM decoded (a 7-minute 360p clip is ~8 GB).
    # Keep every k-th frame so the in-memory clip stays under the budget; the
    # effective fps is divided accordingly, which the extractor handles — its
    # timing comes from scroll geometry, not from a fixed frame rate.
    step = 1
    n_est = cap.get(cv2.CAP_PROP_FRAME_COUNT) or 0
    w = cap.get(cv2.CAP_PROP_FRAME_WIDTH) or 0
    h = cap.get(cv2.CAP_PROP_FRAME_HEIGHT) or 0

    # ~360p is plenty for the extractor (falling bars just need to be legible);
    # halving 720p/1080p sources quarters/divides-by-9 the memory per frame,
    # which the budget below converts into far better *temporal* resolution.
    shrink = max(1, int(round(w / 640))) if w > 720 else 1

    if n_est > 0 and w > 0 and h > 0:
        budget_bytes = _clip_budget_bytes()
        total = n_est * (w / shrink) * (h / shrink) * 3
        if total > budget_bytes:
            step = int(np.ceil(total / budget_bytes))

    frames: list[np.ndarray] = []
    for i, f in enumerate(_iter_capture(cap)):
        if i % step == 0:
            if shrink > 1:
                f = cv2.resize(
                    f,
                    (f.shape[1] // shrink, f.shape[0] // shrink),
                    interpolation=cv2.INTER_AREA,
                )
            frames.append(f)
    cap.release()
    return frames, float(fps) / step


# ~2 GB of decoded frames; beyond this the loader decimates (see load_frames).
_MAX_CLIP_BYTES = 2 * 1024**3


def _clip_budget_bytes() -> int:
    """Frame-memory budget: a third of the process's address-space cap when one
    is set (see ``extract._limit_memory``), else the fixed default.  Low-memory
    machines thus decimate harder instead of failing."""
    try:
        import resource

        soft, _hard = resource.getrlimit(resource.RLIMIT_AS)
        if soft != resource.RLIM_INFINITY:
            return min(_MAX_CLIP_BYTES, max(soft // 3, 256 * 2**20))
    except (ImportError, OSError, ValueError):
        pass
    return _MAX_CLIP_BYTES


def _iter_capture(cap: "cv2.VideoCapture") -> Iterator[np.ndarray]:
    while True:
        ok, frame = cap.read()
        if not ok:
            break
        yield frame


# --------------------------------------------------------------------------- #
# Audio (M6-F) — 16-bit PCM WAV only, via the stdlib (no extra runtime deps).
# --------------------------------------------------------------------------- #
def write_wav(path: str, samples: np.ndarray, sample_rate: int) -> None:
    """Write a mono float array in [-1, 1] as 16-bit PCM WAV (test helper)."""
    clipped = np.clip(np.asarray(samples, dtype=np.float64), -1.0, 1.0)
    pcm = (clipped * 32767.0).astype("<i2")
    with wave.open(path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(int(sample_rate))
        wf.writeframes(pcm.tobytes())


def read_wav(path: str) -> tuple[np.ndarray, int]:
    """Read a WAV file to ``(mono float32 in [-1, 1], sample_rate)``.

    Multi-channel audio is downmixed to mono so the transcriber sees one track.
    """
    with wave.open(path, "rb") as wf:
        n_channels = wf.getnchannels()
        sampwidth = wf.getsampwidth()
        sample_rate = wf.getframerate()
        raw = wf.readframes(wf.getnframes())
    if sampwidth != 2:
        raise ValueError(f"only 16-bit PCM WAV is supported (got {sampwidth * 8}-bit)")
    data = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    if n_channels > 1:
        data = data.reshape(-1, n_channels).mean(axis=1)
    return data, int(sample_rate)
