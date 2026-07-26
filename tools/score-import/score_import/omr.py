"""Optical music recognition stage — scan or PDF → MusicXML (M13-B).

This module **selects and drives an existing engine**. It does not read notation
itself: there is no model here, no training, no vision code and deliberately no
LLM (a hallucinated bar is worse than an OMR error, because it is plausible
rather than obviously broken). Its whole job is to produce a MusicXML file and
hand it to M13-A's converter, which is why the Rust side still sees exactly one
sidecar contract.

An OMR engine is an **optional system dependency**, handled the way ffmpeg is on
the video path and the fetch hook is on the URL path: resolved from an env
override, then from ``PATH``, and when nothing is found the import fails with a
message naming every option. It is never vendored, never auto-downloaded, never
required to build, and never run in CI.

Resolution order (:func:`resolve_engine`):

1. ``$ROCKCRAFT_OMR_CMD`` — an explicit command, invoked as
   ``<cmd> <scan> <out.musicxml>``, for engines or wrappers we don't know about.
2. ``audiveris`` on ``PATH`` — the strongest open engine; needs a JVM, reads PDFs
   whole, exports compressed ``.mxl``.
3. ``oemer`` on ``PATH`` — pure-Python fallback, ``pip install oemer``; weaker on
   dense scores and **single-image only**, so a PDF is rasterized page by page
   and the pages are concatenated.
4. Nothing → :class:`EngineMissing`.
"""

from __future__ import annotations

import copy
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import zipfile
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Callable, Optional, Sequence

from music21 import converter, metadata, stream

from .schema import EXTRACTOR_VERSION

#: Inputs that need OMR rather than a direct parse. Everything else is a score
#: file and goes straight to M13-A's converter.
SCAN_EXTENSIONS = frozenset({".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp"})

ENV_ENGINE = "ROCKCRAFT_OMR_CMD"
ENV_PDFTOPPM = "ROCKCRAFT_PDFTOPPM"

#: Resolution of rasterized PDF pages, in DPI. 300 is the floor most OMR engines
#: document for reliable staff detection.
RASTER_DPI = 300

#: Seconds to wait for an engine's ``--version`` probe. The probe is a nicety —
#: a hung or missing one must never delay the actual transcription.
VERSION_PROBE_TIMEOUT = 15

MISSING_ENGINE_MESSAGE = f"""no optical music recognition engine found — a scan or PDF cannot be imported without one.

Install any ONE of these (all optional; every other import path works without them):

  1. ${ENV_ENGINE} — point it at any engine or wrapper you already have.
     It is invoked as `<cmd> <scan-file> <output.musicxml>` and must write
     MusicXML to that second path.

  2. Audiveris — the strongest open engine; needs a JVM.
     https://github.com/Audiveris/audiveris — install it and put `audiveris` on PATH.

  3. oemer — pure Python, weaker on dense piano scores: `pip install oemer`.
     Single-image only, so PDFs are rasterized page by page (needs `pdftoppm`
     from poppler-utils, or ${ENV_PDFTOPPM}).

See docs/IMPORT.md § Scanned sheet music (OMR)."""


class OmrError(RuntimeError):
    """An OMR stage failure the CLI turns into a non-zero exit."""


class EngineMissing(OmrError):
    """No engine is installed. Actionable, never a crash and never a fallback."""


def is_scan(path: str | os.PathLike[str]) -> bool:
    """True when `path` is a scan or PDF and therefore needs the OMR stage."""
    return Path(path).suffix.lower() in SCAN_EXTENSIONS


# ── Engine resolution ────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Engine:
    """A resolved OMR engine and how to drive it."""

    #: ``"custom"`` / ``"audiveris"`` / ``"oemer"``.
    name: str
    #: The command and any fixed arguments, ready to extend with per-run paths.
    argv: tuple[str, ...]
    #: Whether the engine accepts a PDF directly. A single-image engine forces
    #: the rasterize-and-concatenate path, whose page joins are where measures
    #: get lost — so this being ``False`` earns a warning on stderr.
    handles_pdf: bool
    #: Arguments that make the engine print its version, if it has such a flag.
    version_argv: tuple[str, ...] = ()
    #: Version string discovered by :func:`probe_version`, if any.
    version: Optional[str] = None

    def label(self) -> str:
        """Engine identity for ``SourceMeta.extractor_version``.

        e.g. ``"omr-audiveris-5.3+score-import-0.1"`` — the engine *and* the
        converter, so a chart that reads wrong is traceable to whichever of the
        two produced it.
        """
        engine = f"omr-{self.name}"
        if self.version:
            engine += f"-{self.version}"
        return f"{engine}+{EXTRACTOR_VERSION}"


def resolve_engine(
    *,
    env: Optional[dict[str, str]] = None,
    which: Callable[[str], Optional[str]] = shutil.which,
) -> Engine:
    """Resolve the OMR engine to use, or raise :class:`EngineMissing`.

    `env` and `which` are injected so the whole resolution order is testable
    against a stubbed ``PATH`` with no engine installed anywhere.
    """
    environ = os.environ if env is None else env

    custom = (environ.get(ENV_ENGINE) or "").strip()
    if custom:
        # Split like a shell so the override can carry its own flags. Not checked
        # against PATH: the operator named it explicitly, and a bad command fails
        # loudly with the engine's own message when it runs. Assumed to handle
        # whatever it is given, PDFs included.
        return Engine(name="custom", argv=tuple(shlex.split(custom)), handles_pdf=True)

    audiveris = which("audiveris")
    if audiveris:
        return Engine(
            name="audiveris",
            argv=(audiveris,),
            handles_pdf=True,
            version_argv=("-version",),
        )

    oemer = which("oemer")
    if oemer:
        return Engine(
            name="oemer",
            argv=(oemer,),
            handles_pdf=False,
            version_argv=("--version",),
        )

    raise EngineMissing(MISSING_ENGINE_MESSAGE)


def probe_version(engine: Engine) -> Engine:
    """Best-effort: return `engine` with its reported version filled in.

    Purely for traceability, so every failure mode — no version flag, a
    non-zero exit, a hang, no such binary — returns the engine unchanged rather
    than derailing an import that would otherwise have worked.
    """
    if not engine.version_argv:
        return engine
    try:
        result = subprocess.run(
            [*engine.argv, *engine.version_argv],
            capture_output=True,
            text=True,
            timeout=VERSION_PROBE_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return engine
    for token in f"{result.stdout} {result.stderr}".split():
        stripped = token.strip("v,;()")
        if stripped and stripped[0].isdigit():
            return replace(engine, version=stripped)
    return engine


# ── Running the engine ───────────────────────────────────────────────────────


def _log(message: str) -> None:
    """Diagnostics go to stderr: stdout belongs to the chart JSON alone."""
    print(message, file=sys.stderr)


def transcribe(
    scan: str | os.PathLike[str],
    out_path: str | os.PathLike[str],
    *,
    engine: Optional[Engine] = None,
    log: Callable[[str], None] = _log,
) -> Engine:
    """Transcribe `scan` to MusicXML at `out_path`; return the engine that ran.

    Resolves an engine if none is given, rasterizes a PDF when the engine only
    reads images, and concatenates the resulting pages. Raises
    :class:`EngineMissing` when nothing is installed and :class:`OmrError` when
    the engine runs but produces nothing usable.
    """
    scan_path = Path(scan)
    if not scan_path.exists():
        raise OmrError(f"file not found: {scan_path}")
    if engine is None:
        engine = probe_version(resolve_engine())

    log(f"using OMR engine {engine.name}: {' '.join(engine.argv)}")
    log(
        "OMR is an inference step, not a transform: clean engraved scores "
        "transcribe well, handwritten and dense piano scores produce errors. "
        "Review the result before trusting it."
    )

    out = Path(out_path)
    out.parent.mkdir(parents=True, exist_ok=True)

    if scan_path.suffix.lower() == ".pdf" and not engine.handles_pdf:
        _transcribe_pdf_by_page(scan_path, out, engine, log)
    else:
        _run_engine(engine, scan_path, out, log)
    return engine


def _run_engine(engine: Engine, source: Path, out: Path, log: Callable[[str], None]) -> None:
    """Run `engine` over one input file, leaving plain MusicXML at `out`.

    The two shapes: a custom command is told where to write and is trusted to have
    written it; the engines we know about are given a scratch directory and their
    export is located and normalized afterwards, because neither lets us name the
    output file directly.
    """
    if engine.name == "custom":
        # The custom-command contract: <cmd> <scan> <out>, mirroring the fetch
        # hook's <cmd> <url> <target>.
        _spawn([*engine.argv, str(source), str(out)], log)
        if not out.exists():
            raise OmrError(
                f"${ENV_ENGINE} exited successfully but wrote no MusicXML to {out}; "
                "the command must write MusicXML to the second argument it is given"
            )
        return

    with _work_dir(out) as work:
        if engine.name == "audiveris":
            argv = [*engine.argv, "-batch", "-export", "-output", str(work), "--", str(source)]
        else:  # oemer
            argv = [*engine.argv, "-o", str(work), str(source)]
        _spawn(argv, log)

        produced = _find_export(work)
        if produced is None:
            raise OmrError(
                f"{engine.name} produced no MusicXML under {work}; "
                "the scan may be unreadable (try a higher-resolution image)"
            )
        _normalize_export(produced, out)


def _spawn(argv: Sequence[str], log: Callable[[str], None]) -> None:
    """Run `argv`, forwarding its output to stderr and raising on failure.

    Output is buffered rather than streamed on purpose: the Rust pipeline reads
    this whole process with one `Command::output()`, so nothing reaches the
    frontend until the sidecar exits anyway, and streaming here would buy the user
    nothing while complicating the failure path.
    """
    try:
        result = subprocess.run(argv, capture_output=True, text=True, check=False)
    except OSError as exc:
        raise OmrError(f"could not run {argv[0]!r}: {exc}") from exc
    for line in f"{result.stdout}\n{result.stderr}".splitlines():
        if line.strip():
            log(line.rstrip())
    if result.returncode != 0:
        raise OmrError(f"{Path(argv[0]).name} exited with status {result.returncode}")


#: Files an engine may leave behind, best first: plain MusicXML beats the zipped
#: container only because it needs no unpacking — both are read fine.
_EXPORT_GLOBS = ("*.musicxml", "*.xml", "*.mxl")


def _find_export(work: Path) -> Optional[Path]:
    """The MusicXML an engine wrote somewhere under `work`.

    Searched recursively because Audiveris nests its export inside a per-book
    directory whose name it derives from the input.
    """
    for pattern in _EXPORT_GLOBS:
        matches = sorted(work.rglob(pattern))
        if matches:
            return matches[0]
    return None


def _normalize_export(produced: Path, out: Path) -> None:
    """Copy `produced` to `out` as plain MusicXML, unzipping ``.mxl`` if needed."""
    if produced.suffix.lower() == ".mxl":
        out.write_bytes(unpack_mxl(produced))
    else:
        out.write_bytes(produced.read_bytes())


def unpack_mxl(path: str | os.PathLike[str]) -> bytes:
    """The MusicXML inside a compressed ``.mxl`` container.

    ``.mxl`` is a zip whose ``META-INF/container.xml`` names the real score file.
    Unpacked here rather than handed to ``music21`` as-is so that everything
    downstream — including the ``--out`` file a human debugs with — is plain,
    readable XML, and so no zip ever reaches the repository's content guard.
    """
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        root = _container_rootfile(archive, names)
        if root is None:
            candidates = [
                n for n in names if n.lower().endswith((".xml", ".musicxml")) and "META-INF" not in n
            ]
            if not candidates:
                raise OmrError(f"{path} is not a readable .mxl container (no score inside)")
            root = sorted(candidates)[0]
        return archive.read(root)


def _container_rootfile(archive: zipfile.ZipFile, names: Sequence[str]) -> Optional[str]:
    """The score path ``META-INF/container.xml`` points at, if it is present."""
    if "META-INF/container.xml" not in names:
        return None
    container = archive.read("META-INF/container.xml").decode("utf-8", "replace")
    match = re.search(r'full-path\s*=\s*"([^"]+)"', container)
    if match and match.group(1) in names:
        return match.group(1)
    return None


class _work_dir:
    """A scratch directory beside `out`, removed when the block exits."""

    def __init__(self, out: Path) -> None:
        self._out = out

    def __enter__(self) -> Path:
        self._tmp = tempfile.TemporaryDirectory(dir=str(self._out.parent), prefix="omr-")
        return Path(self._tmp.name)

    def __exit__(self, *exc: object) -> None:
        self._tmp.cleanup()


# ── Multi-page PDFs on a single-image engine ─────────────────────────────────


def _transcribe_pdf_by_page(
    pdf: Path, out: Path, engine: Engine, log: Callable[[str], None]
) -> None:
    """Rasterize `pdf`, transcribe each page, concatenate the results.

    Only reached when the resolved engine cannot read a PDF itself. Loudly
    warned about, because page-boundary joins are exactly where measures go
    missing: a system split across two pages becomes two partial systems, and
    nothing downstream can tell that from the real thing.
    """
    log(
        f"{engine.name} reads single images only, so this PDF is rasterized at "
        f"{RASTER_DPI} DPI and its pages are transcribed separately, then "
        "concatenated in page order. Measures that straddle a page boundary are "
        "the likeliest thing to be wrong — check them first."
    )
    with _work_dir(out) as work:
        pages = rasterize_pdf(pdf, work, log=log)
        log(f"rasterized {len(pages)} page(s)")
        page_scores = []
        for index, page in enumerate(pages, start=1):
            page_xml = work / f"page-{index:04d}.musicxml"
            log(f"transcribing page {index}/{len(pages)}")
            _run_engine(engine, page, page_xml, log)
            page_scores.append(converter.parse(str(page_xml)))
        merged = concatenate_scores(page_scores, log=log)
        merged.write("musicxml", fp=str(out))


def rasterize_pdf(
    pdf: Path,
    out_dir: Path,
    *,
    env: Optional[dict[str, str]] = None,
    which: Callable[[str], Optional[str]] = shutil.which,
    log: Callable[[str], None] = _log,
) -> list[Path]:
    """Render `pdf` to one PNG per page in `out_dir`, sorted by page number.

    Uses ``pdftoppm`` (poppler-utils), overridable with ``$ROCKCRAFT_PDFTOPPM`` —
    another optional system dependency rather than a new Python one, matching how
    ffmpeg is treated on the video path.
    """
    environ = os.environ if env is None else env
    cmd = (environ.get(ENV_PDFTOPPM) or "").strip() or which("pdftoppm")
    if not cmd:
        raise OmrError(
            "rasterizing a PDF needs `pdftoppm` (poppler-utils), because the "
            "resolved OMR engine reads single images only. Install poppler-utils, "
            f"set ${ENV_PDFTOPPM}, or install Audiveris — it reads PDFs directly."
        )
    _spawn([cmd, "-r", str(RASTER_DPI), "-png", str(pdf), str(out_dir / "page")], log)
    pages = sorted(out_dir.glob("page*.png"))
    if not pages:
        raise OmrError(f"pdftoppm produced no pages from {pdf}")
    return pages


def concatenate_scores(
    pages: Sequence[stream.Score], *, log: Callable[[str], None] = _log
) -> stream.Score:
    """Join per-page scores end to end, part by part, in page order.

    Pure over parsed scores — no engine, no PDF — so the join itself is testable.
    Parts are matched **by index**, which is all a page-at-a-time engine gives
    us: page 2's first staff is assumed to continue page 1's first staff. A page
    that reports a different number of staves breaks that assumption, so it is
    warned about rather than quietly stitched.
    """
    merged = stream.Score()
    if not pages:
        return merged

    first_meta = pages[0].metadata
    merged.insert(0, copy.deepcopy(first_meta) if first_meta is not None else metadata.Metadata())

    widths = [len(_page_parts(page)) for page in pages]
    if len(set(widths)) > 1:
        log(
            f"pages report differing staff counts {widths}; parts are joined by "
            "index, so a page with fewer staves leaves a gap in the part it is "
            "missing — check the page boundaries"
        )

    for index in range(max(widths)):
        part = stream.Part()
        number = 1
        for page in pages:
            page_parts = _page_parts(page)
            if index >= len(page_parts):
                continue
            for measure in page_parts[index].getElementsByClass(stream.Measure):
                clone = copy.deepcopy(measure)
                clone.number = number
                number += 1
                part.append(clone)
        merged.insert(0, part)
    return merged


def _page_parts(page: stream.Score) -> list[stream.Stream]:
    parts = list(page.parts) if hasattr(page, "parts") else []
    return parts if parts else [page]
