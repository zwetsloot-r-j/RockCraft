"""Tests for the M13-B OMR stage.

**No OMR engine runs here, and none runs in CI.** What is tested is everything
*around* the engine — which is where the bugs that matter live:

- engine resolution and the actionable error when nothing is installed,
- the routing decision (a scan goes through OMR, a score file does not),
- unpacking Audiveris's compressed export,
- joining rasterized PDF pages,
- and one genuine end-to-end run through `convert.py` against a **stub engine**
  installed via ``$ROCKCRAFT_OMR_CMD``, which exercises the real resolution, the
  real conversion and the real summary without any engine being present.

A run against a real scan is a manual local check — see the ``omr`` marker.
"""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
import zipfile

import pytest
from music21 import meter, note, stream

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(ROOT))
FIXTURES = os.path.join(REPO, "fixtures", "score")
sys.path.insert(0, ROOT)

from score_import import confidence as conf  # noqa: E402
from score_import import omr  # noqa: E402
from score_import.convert import convert_input  # noqa: E402
from score_import.schema import EXTRACTOR_VERSION  # noqa: E402


def which_stub(*available: str):
    """A ``shutil.which`` that finds only `available` — a stubbed ``PATH``."""
    return lambda name: f"/usr/bin/{name}" if name in available else None


# ── Which inputs need OMR at all ─────────────────────────────────────────────


def test_scans_need_omr_and_score_files_do_not():
    for scan in ("page.pdf", "page.PNG", "photo.jpg", "shot.jpeg", "scan.tif", "x.tiff", "y.bmp"):
        assert omr.is_scan(scan), scan
    for score in ("song.musicxml", "song.xml", "song.mxl", "song.abc", "song.krn", "song.mid"):
        assert not omr.is_scan(score), score


# ── Engine resolution ────────────────────────────────────────────────────────


def test_the_env_override_wins_over_everything_on_path():
    engine = omr.resolve_engine(
        env={omr.ENV_ENGINE: "my-omr --flag"},
        which=which_stub("audiveris", "oemer"),
    )
    assert engine.name == "custom"
    assert engine.argv == ("my-omr", "--flag"), "the override keeps its own flags"
    assert engine.handles_pdf, "an explicit command is trusted with whatever it is given"


def test_audiveris_is_preferred_when_no_override_is_set():
    engine = omr.resolve_engine(env={}, which=which_stub("audiveris", "oemer"))
    assert engine.name == "audiveris"
    assert engine.handles_pdf, "Audiveris reads PDFs whole — no page splitting"


def test_oemer_is_the_fallback():
    engine = omr.resolve_engine(env={}, which=which_stub("oemer"))
    assert engine.name == "oemer"
    assert not engine.handles_pdf, "oemer is single-image, so PDFs must be rasterized"


def test_an_empty_override_falls_through_rather_than_resolving_to_nothing():
    engine = omr.resolve_engine(env={omr.ENV_ENGINE: "   "}, which=which_stub("oemer"))
    assert engine.name == "oemer"


def test_with_no_engine_the_error_names_all_three_options():
    with pytest.raises(omr.EngineMissing) as excinfo:
        omr.resolve_engine(env={}, which=which_stub())
    message = str(excinfo.value)
    assert omr.ENV_ENGINE in message
    assert "Audiveris" in message
    assert "oemer" in message
    assert "docs/IMPORT.md" in message


def test_a_missing_engine_is_an_omr_error_not_a_crash():
    """The CLI catches `OmrError`; `EngineMissing` must stay inside that family so
    an absent optional dependency exits 2 with a message rather than a traceback."""
    assert issubclass(omr.EngineMissing, omr.OmrError)


# ── Engine identity in the extractor version ─────────────────────────────────


def test_the_engine_and_the_converter_are_both_named_in_the_version():
    engine = omr.Engine(name="audiveris", argv=("audiveris",), handles_pdf=True, version="5.3")
    assert engine.label() == f"omr-audiveris-5.3+{EXTRACTOR_VERSION}"


def test_a_version_free_engine_still_names_itself():
    engine = omr.Engine(name="oemer", argv=("oemer",), handles_pdf=False)
    assert engine.label() == f"omr-oemer+{EXTRACTOR_VERSION}"


def test_the_version_probe_reads_the_engines_own_output(tmp_path):
    stub = tmp_path / "printver.py"
    stub.write_text("print('Audiveris 5.3.1')\n")
    engine = omr.Engine(
        name="audiveris",
        argv=(sys.executable, str(stub)),
        handles_pdf=True,
        version_argv=("-version",),
    )
    assert omr.probe_version(engine).version == "5.3.1"


def test_a_failing_version_probe_is_not_an_error():
    """Traceability is a nicety; it must never fail an import that would work."""
    engine = omr.Engine(
        name="audiveris",
        argv=("definitely-not-an-executable-38f1",),
        handles_pdf=True,
        version_argv=("-version",),
    )
    assert omr.probe_version(engine).version is None


# ── Audiveris's compressed export ────────────────────────────────────────────

CONTAINER = """<?xml version="1.0"?>
<container><rootfiles>
  <rootfile full-path="score.xml" media-type="application/vnd.recordare.musicxml+xml"/>
</rootfiles></container>"""


def test_a_compressed_export_is_unpacked_to_plain_musicxml(tmp_path):
    """`.mxl` is a zip. Everything downstream — including whatever a human opens
    to debug a bad import — must see readable XML, and no zip may ever end up
    where the repository's content guard would have to reason about it."""
    mxl = tmp_path / "export.mxl"
    with zipfile.ZipFile(mxl, "w") as archive:
        archive.writestr("META-INF/container.xml", CONTAINER)
        archive.writestr("score.xml", "<score-partwise/>")
        archive.writestr("decoy.xml", "<not-the-score/>")
    assert omr.unpack_mxl(mxl) == b"<score-partwise/>", "the container names the real score"


def test_a_container_less_export_falls_back_to_the_xml_inside(tmp_path):
    mxl = tmp_path / "export.mxl"
    with zipfile.ZipFile(mxl, "w") as archive:
        archive.writestr("whatever.musicxml", "<score-partwise/>")
    assert omr.unpack_mxl(mxl) == b"<score-partwise/>"


def test_an_export_with_no_score_inside_is_a_clear_failure(tmp_path):
    mxl = tmp_path / "export.mxl"
    with zipfile.ZipFile(mxl, "w") as archive:
        archive.writestr("readme.txt", "nothing useful")
    with pytest.raises(omr.OmrError, match="not a readable .mxl"):
        omr.unpack_mxl(mxl)


# ── Rasterizing a PDF for a single-image engine ──────────────────────────────


def test_rasterizing_without_poppler_says_what_to_install(tmp_path):
    with pytest.raises(omr.OmrError) as excinfo:
        omr.rasterize_pdf(tmp_path / "scan.pdf", tmp_path, env={}, which=which_stub(), log=lambda _: None)
    message = str(excinfo.value)
    assert "pdftoppm" in message
    assert omr.ENV_PDFTOPPM in message
    assert "Audiveris" in message, "the simpler fix is an engine that reads PDFs"


# ── Joining rasterized pages ─────────────────────────────────────────────────


def page(*bars: int, staves: int = 1) -> stream.Score:
    """A page score with `staves` staves, each holding ``len(bars)`` measures."""
    score = stream.Score()
    for _ in range(staves):
        part = stream.Part()
        for index, number in enumerate(bars):
            measure = stream.Measure(number=number)
            if index == 0:
                measure.insert(0, meter.TimeSignature("4/4"))
            measure.insert(0, note.Note("C4", quarterLength=4.0))
            part.append(measure)
        score.insert(0, part)
    return score


def test_pages_are_joined_in_order_and_renumbered_continuously():
    merged = omr.concatenate_scores([page(1, 2), page(1, 2, 3)], log=lambda _: None)
    measures = list(merged.parts[0].getElementsByClass(stream.Measure))
    assert [m.number for m in measures] == [1, 2, 3, 4, 5], "page 2 continues page 1's numbering"
    assert len(merged.flatten().notes) == 5


def test_joining_preserves_every_staff():
    merged = omr.concatenate_scores([page(1, staves=2), page(1, staves=2)], log=lambda _: None)
    assert len(merged.parts) == 2
    assert all(len(list(p.getElementsByClass(stream.Measure))) == 2 for p in merged.parts)


def test_a_page_with_a_different_staff_count_is_warned_about():
    """Parts are matched by index — all a page-at-a-time engine allows — so a page
    that disagrees about how many staves there are cannot be joined silently."""
    logged: list[str] = []
    merged = omr.concatenate_scores([page(1, staves=2), page(1, staves=1)], log=logged.append)
    assert len(merged.parts) == 2
    assert any("staff counts" in line for line in logged)


def test_joining_nothing_is_not_a_crash():
    assert len(omr.concatenate_scores([], log=lambda _: None).flatten().notes) == 0


# ── Routing: convert_input decides, and only a scan pays for OMR ──────────────


def test_a_score_file_never_touches_the_omr_stage(monkeypatch):
    calls: list[str] = []
    monkeypatch.setattr(
        omr, "transcribe", lambda *a, **k: calls.append("ran") or omr.Engine("stub", (), True)
    )
    chart, report = convert_input(os.path.join(FIXTURES, "single_staff_scale.musicxml"))
    assert calls == [], "a .musicxml is stated, not inferred — no engine may run"
    assert {n.confidence for n in chart.notes} == {1.0}
    assert report.confidence is None
    assert chart.source.extractor_version == EXTRACTOR_VERSION


def test_a_pdf_routes_through_the_omr_stage(monkeypatch, tmp_path):
    """The stub stands in for an engine: it writes MusicXML where it is asked to,
    which is the entire contract `convert_input` depends on."""
    calls: list[tuple[str, str]] = []

    def fake_transcribe(scan, out_path, *, engine=None, log=None):
        calls.append((str(scan), str(out_path)))
        with open(os.path.join(FIXTURES, "single_staff_scale.musicxml"), "rb") as src:
            with open(out_path, "wb") as dst:
                dst.write(src.read())
        return omr.Engine(name="audiveris", argv=("audiveris",), handles_pdf=True, version="5.3")

    monkeypatch.setattr(omr, "transcribe", fake_transcribe)
    scan = tmp_path / "scan.pdf"
    scan.write_bytes(b"%PDF-1.4 not really a pdf")

    chart, report = convert_input(str(scan))
    assert len(calls) == 1 and calls[0][0] == str(scan)
    assert not os.path.exists(calls[0][1]), "the intermediate MusicXML is not left behind"
    assert {n.confidence for n in chart.notes} == {conf.CLEAN}, "never 1.0 on an OMR path"
    assert report.confidence is not None
    assert chart.source.extractor_version == f"omr-audiveris-5.3+{EXTRACTOR_VERSION}"


# ── End to end through the CLI, against a stub engine ────────────────────────


def install_stub_engine(tmp_path, fixture: str) -> dict[str, str]:
    """Env installing a stub ``$ROCKCRAFT_OMR_CMD`` that "transcribes" `fixture`.

    Exercises the real resolution path and the real custom-command contract
    (``<cmd> <scan> <out>``) without an engine anywhere on the machine.
    """
    stub = tmp_path / "stub_engine.py"
    stub.write_text("import shutil, sys\nshutil.copyfile(sys.argv[1], sys.argv[3])\n")
    command = " ".join(
        shlex.quote(part) for part in (sys.executable, str(stub), os.path.join(FIXTURES, fixture))
    )
    env = dict(os.environ)
    env[omr.ENV_ENGINE] = command
    return env


def run_cli(env, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, os.path.join(ROOT, "convert.py"), *args],
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )


def test_a_scan_import_emits_pure_json_on_stdout_and_the_summary_on_stderr(tmp_path):
    scan = tmp_path / "page.pdf"
    scan.write_bytes(b"%PDF-1.4")
    result = run_cli(install_stub_engine(tmp_path, "single_staff_scale.musicxml"), "--in", str(scan), "--out", "-")

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)  # would raise if any diagnostic leaked here
    assert len(payload["notes"]) == 8
    assert {n["confidence"] for n in payload["notes"]} == {conf.CLEAN}
    assert payload["source"]["extractor_version"].startswith("omr-custom")

    assert f"{conf.SUMMARY_PREFIX}imported 8 notes, 0 flagged" in result.stderr
    assert "using OMR engine custom" in result.stderr
    assert "Review the result before trusting it" in result.stderr


def test_the_stderr_summary_counts_agree_with_the_emitted_notes(tmp_path):
    """The counts are the whole review affordance in this task, so they have to be
    the same numbers the chart carries — not an independently computed guess."""
    scan = tmp_path / "page.png"
    scan.write_bytes(b"\x89PNG")
    result = run_cli(install_stub_engine(tmp_path, "grace_note.musicxml"), "--in", str(scan), "--out", "-")

    assert result.returncode == 0, result.stderr
    notes = json.loads(result.stdout)["notes"]
    flagged = sum(1 for n in notes if n["confidence"] < conf.CLEAN)
    expected = f"{conf.SUMMARY_PREFIX}imported {len(notes)} notes, {flagged} flagged"
    assert expected in result.stderr
    if flagged:
        assert "review in the editor" in result.stderr
        assert "suspect measures" in result.stderr


def test_a_scan_with_no_engine_installed_fails_with_the_three_options(tmp_path):
    """The acceptance case: nothing installed, so the import fails actionably —
    and nothing about the build or any other import path is affected."""
    env = dict(os.environ)
    env.pop(omr.ENV_ENGINE, None)
    # An empty PATH is the only reliable way to prove "no engine anywhere".
    env["PATH"] = str(tmp_path / "empty-bin")
    scan = tmp_path / "page.pdf"
    scan.write_bytes(b"%PDF-1.4")

    result = run_cli(env, "--in", str(scan), "--out", "-")
    assert result.returncode == 2
    assert result.stdout.strip() == "", "a failed import writes no chart"
    for expected in (omr.ENV_ENGINE, "Audiveris", "oemer", "docs/IMPORT.md"):
        assert expected in result.stderr


def test_a_broken_stub_engine_fails_loudly_rather_than_writing_an_empty_chart(tmp_path):
    """An engine that exits 0 without producing MusicXML is the nastiest failure
    mode: it must not become a silently empty chart."""
    stub = tmp_path / "silent.py"
    stub.write_text("import sys\nsys.exit(0)\n")
    env = dict(os.environ)
    env[omr.ENV_ENGINE] = " ".join(shlex.quote(p) for p in (sys.executable, str(stub)))
    scan = tmp_path / "page.pdf"
    scan.write_bytes(b"%PDF-1.4")

    result = run_cli(env, "--in", str(scan), "--out", "-")
    assert result.returncode == 2
    assert result.stdout.strip() == ""
    assert "wrote no MusicXML" in result.stderr


def test_the_standalone_omr_cli_rejects_a_score_file(tmp_path):
    """`omr.py` is the debugging half of the pipeline; handing it a `.musicxml`
    is a mistake worth naming rather than an engine run worth attempting."""
    result = subprocess.run(
        [
            sys.executable,
            os.path.join(ROOT, "omr.py"),
            "--in",
            os.path.join(FIXTURES, "single_note.musicxml"),
            "--out",
            str(tmp_path / "out.musicxml"),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 2
    assert "is not a scan" in result.stderr


def test_the_standalone_omr_cli_writes_musicxml(tmp_path):
    scan = tmp_path / "page.pdf"
    scan.write_bytes(b"%PDF-1.4")
    out = tmp_path / "transcribed.musicxml"
    result = subprocess.run(
        [sys.executable, os.path.join(ROOT, "omr.py"), "--in", str(scan), "--out", str(out)],
        capture_output=True,
        text=True,
        check=False,
        env=install_stub_engine(tmp_path, "single_note.musicxml"),
    )
    assert result.returncode == 0, result.stderr
    assert out.exists() and b"score-partwise" in out.read_bytes()
    assert "omr-custom" in result.stderr


# ── Manual local check ───────────────────────────────────────────────────────


@pytest.mark.omr
def test_a_real_engine_transcribes_a_real_scan(tmp_path):
    """Deselected by default; the only test here that needs an engine and a scan.

    Run it locally against your own material — never a committed one:

        ROCKCRAFT_OMR_SCAN=~/scans/my-page.pdf \\
          python -m pytest -m omr tools/score-import/

    Expect errors on handwritten or dense piano scores: the assertion is that the
    pipeline produces a reviewable chart, not a correct one.
    """
    scan = os.environ.get("ROCKCRAFT_OMR_SCAN")
    if not scan:
        pytest.skip("set ROCKCRAFT_OMR_SCAN to a local scan to run this")
    chart, report = convert_input(scan)
    assert chart.notes, "a real scan should yield at least one note"
    assert report.confidence is not None
    assert all(n.confidence is not None and n.confidence <= conf.CLEAN for n in chart.notes)
    print(report.confidence.summary_line(), file=sys.stderr)
