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
    if n_est > 0 and w > 0 and h > 0:
        budget_bytes = _clip_budget_bytes()
        total = n_est * w * h * 3
        if total > budget_bytes:
            step = int(np.ceil(total / budget_bytes))

    frames: list[np.ndarray] = []
    for i, f in enumerate(_iter_capture(cap)):
        if i % step == 0:
            frames.append(f)
    cap.release()
    return frames, float(fps) / step


def video_info(
    path: str, fps_override: Optional[float] = None
) -> tuple[float, int, int, int]:
    """``(fps, frame_count, width, height)`` for a video file (no decode)."""
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise FileNotFoundError(f"could not open video: {path}")
    fps = fps_override or cap.get(cv2.CAP_PROP_FPS) or 30.0
    n = int(cap.get(cv2.CAP_PROP_FRAME_COUNT) or 0)
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH) or 0)
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT) or 0)
    cap.release()
    return float(fps), n, w, h


def sparse_sample(
    path: str, max_frames: int, fps_override: Optional[float] = None
) -> tuple[list[np.ndarray], float]:
    """A small, evenly-spread set of frames spanning the whole clip.

    Used by the streaming extractor for global setup (background plate, keyboard
    calibration, scroll speed, dead-zone / text-band maps) — none of which need
    full temporal density.  Returns ``(frames, effective_fps)`` where the fps is
    the source rate divided by the sampling step, so frame-pair timing (e.g.
    scroll estimation) stays correct.

    Uses frame *seeking* (not a full decode) to grab only the sampled frames, so
    setup costs a few hundred decodes rather than the whole clip — the dense
    streaming pass is the one full decode.
    """
    fps, n, _w, _h = video_info(path, fps_override)
    step = 1 if n <= 0 else max(1, int(np.ceil(n / max(1, max_frames))))
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise FileNotFoundError(f"could not open video: {path}")
    frames: list[np.ndarray] = []
    if n > 0:
        for idx in range(0, n, step):
            cap.set(cv2.CAP_PROP_POS_FRAMES, idx)
            ok, f = cap.read()
            if ok:
                frames.append(f)
    else:  # unknown length — fall back to a sequential keep-every-step pass
        for i, f in enumerate(_iter_capture(cap)):
            if i % step == 0:
                frames.append(f)
    cap.release()
    return frames, float(fps) / step


def read_burst(path: str, start_frame: int, count: int) -> list[np.ndarray]:
    """Read up to ``count`` *consecutive* frames starting near ``start_frame``.

    Scroll estimation cross-correlates adjacent frames, so it needs a short
    full-rate burst (the sparse global sample is far too coarse — its frames are
    seconds apart, well beyond a bar's per-frame travel).  The exact start frame
    is unimportant (we only need consecutive frames), so an approximate keyframe
    seek is fine.
    """
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise FileNotFoundError(f"could not open video: {path}")
    if start_frame > 0:
        cap.set(cv2.CAP_PROP_POS_FRAMES, start_frame)
    out: list[np.ndarray] = []
    for _ in range(count):
        ok, f = cap.read()
        if not ok:
            break
        out.append(f)
    cap.release()
    return out


def stream_frames(path: str) -> Iterator[np.ndarray]:
    """Yield every frame of a video in order at full rate (one decode in flight).

    The caller buffers only the few frames it needs (e.g. a coherence look-ahead),
    so a multi-minute clip streams at native fps without ever holding it all in
    RAM — this is what lets the extractor see fast passages the decimating
    :func:`load_frames` drops.
    """
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise FileNotFoundError(f"could not open video: {path}")
    try:
        yield from _iter_capture(cap)
    finally:
        cap.release()


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
