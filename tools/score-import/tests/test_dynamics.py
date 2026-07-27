"""Tests for the M13-D dynamics → velocity pass.

Every fixture under ``fixtures/score/`` is *synthetic* — a hand-written scale, a
fabricated hairpin — never a real or copyrighted piece.

Velocities here are asserted **exactly**, with no tolerance: the mapping is a
lookup table and a linear ramp, both fully determined by what the score states.
The one number that carries a contract beyond this module is ``mf`` → 80, which
is ``parser::DEFAULT_VELOCITY``: it is what makes an ``mf``-only score sound
exactly as it did before this pass existed.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(ROOT))
FIXTURES = os.path.join(REPO, "fixtures", "score")
sys.path.insert(0, ROOT)

from score_import.convert import convert_score  # noqa: E402
from score_import.dynamics import DYNAMIC_VELOCITY, MAX_VELOCITY  # noqa: E402
from score_import.schema import Hand  # noqa: E402

#: `crates/import/src/parser.rs`'s `DEFAULT_VELOCITY`, mirrored. If this ever
#: stops matching, an `mf` score changes how it sounds without anyone asking.
DEFAULT_VELOCITY = 80


def fixture(name: str) -> str:
    return os.path.join(FIXTURES, f"{name}.musicxml")


def convert(name: str, **kwargs):
    return convert_score(fixture(name), **kwargs)


def velocities_of(name: str, **kwargs) -> list:
    chart, _ = convert(name, **kwargs)
    return [n.velocity for n in chart.notes]


def warnings_of(report) -> str:
    return " | ".join(report.warnings)


# ── The ladder ───────────────────────────────────────────────────────────────


def test_mf_is_the_rust_default_velocity():
    """The property that keeps an `mf` score sounding exactly as it did."""
    assert DYNAMIC_VELOCITY["mf"] == DEFAULT_VELOCITY


def test_notated_levels_become_their_velocities():
    """A bar marked `p` and a bar marked `ff`, resolved per note."""
    assert velocities_of("dynamics_levels") == [49] * 4 + [112] * 4


# ── Hairpins ─────────────────────────────────────────────────────────────────


def test_crescendo_ramps_from_the_dynamic_before_it_to_the_one_after():
    """`p` → cresc. → `f`: starts at 49, ends at 96, monotonic in between."""
    ramp = velocities_of("dynamics_hairpin")[:4]
    assert ramp[0] == DYNAMIC_VELOCITY["p"] == 49
    assert ramp[-1] == DYNAMIC_VELOCITY["f"] == 96
    assert ramp == sorted(ramp), "a crescendo never gets quieter"
    assert len(set(ramp)) == len(ramp), "every step of the ramp is audible"


def test_the_note_after_a_hairpin_holds_the_dynamic_it_resolved_to():
    assert velocities_of("dynamics_hairpin")[4] == DYNAMIC_VELOCITY["f"]


def test_unresolved_crescendo_ramps_one_level_and_warns():
    """No dynamic after the wedge: ramp `p` → `mp`, and say so rather than
    either ignoring the wedge or inventing an arrival the score never stated."""
    chart, report = convert("dynamics_open_hairpin")
    ramp = [n.velocity for n in chart.notes]
    assert ramp[0] == DYNAMIC_VELOCITY["p"]
    assert ramp[-1] == DYNAMIC_VELOCITY["mp"], "one rung up the ladder"
    assert ramp == sorted(ramp)
    assert "no dynamic resolving it" in warnings_of(report)


# ── Articulations ────────────────────────────────────────────────────────────


def test_accent_and_marcato_stack_on_the_level_and_clamp():
    """`mf` + accent = 95; `fff` + marcato saturates at 127 rather than wrapping."""
    plain_mf, accented_mf, plain_fff, marcato_fff = velocities_of("dynamics_articulations")
    assert plain_mf == DYNAMIC_VELOCITY["mf"] == 80
    assert accented_mf == 95
    assert plain_fff == DYNAMIC_VELOCITY["fff"] == 126
    assert marcato_fff == MAX_VELOCITY == 127


# ── Per-staff independence ───────────────────────────────────────────────────


def test_dynamics_resolve_per_staff_not_across_the_score():
    """A left hand marked `p` under a right hand marked `f` is ordinary piano
    writing — the two must not be flattened onto one curve."""
    chart, _ = convert("dynamics_two_staff")
    right = {n.velocity for n in chart.notes if n.hand is Hand.RIGHT}
    left = {n.velocity for n in chart.notes if n.hand is Hand.LEFT}
    assert right == {DYNAMIC_VELOCITY["f"]}
    assert left == {DYNAMIC_VELOCITY["p"]}


# ── Silence: what the source does not say ────────────────────────────────────


def test_a_score_with_no_dynamics_emits_no_velocities():
    """Not 80 — `None`. `DEFAULT_VELOCITY` still applies downstream, and "the
    source didn't say" stays distinguishable from a stated value."""
    assert velocities_of("single_staff_scale") == [None] * 8


def test_no_dynamics_flag_restores_the_flat_default():
    assert velocities_of("dynamics_levels", read_dynamics=False) == [None] * 8


def test_an_unmarked_score_is_identical_with_and_without_the_pass():
    """The M13-A compatibility guarantee, on the bytes that reach Rust."""
    with_pass = convert("single_staff_scale")[0].to_json(indent=2)
    without = convert("single_staff_scale", read_dynamics=False)[0].to_json(indent=2)
    assert with_pass == without


def test_a_stated_velocity_wins_over_the_notated_dynamic():
    """A velocity the file states outright is the strongest signal there is —
    rule 7's original clause, still honoured now that a level can also speak."""
    from music21 import note as m21note

    from score_import.convert import _velocity
    from score_import.dynamics import StaffDynamics

    under_p = StaffDynamics(marks=((0.0, DYNAMIC_VELOCITY["p"]),))
    stated = m21note.Note("C4")
    stated.volume.velocity = 100
    assert _velocity(stated, under_p, 0.0) == 100
    assert _velocity(m21note.Note("C4"), under_p, 0.0) == DYNAMIC_VELOCITY["p"]


def test_markings_outside_the_ladder_are_dropped_not_mapped():
    """`sf` is an accent and `fp` a shape; neither names a level. Forcing either
    onto a rung would set the loudness of everything after it to a value the
    score never asked for — so they are dropped, and counted."""
    from music21 import dynamics as m21dynamics, note as m21note, stream

    from score_import import dynamics as dyn

    part = stream.Part()
    part.insert(0.0, m21dynamics.Dynamic("sf"))
    part.insert(0.0, m21note.Note("C4"))
    part.insert(1.0, m21dynamics.Dynamic("mf"))
    part.insert(1.0, m21note.Note("D4"))

    dropped: dict[str, int] = {}
    staff = dyn.read(part, drop=lambda category, count: dropped.update({category: count}))

    assert staff.velocity_for(None, 0.0) is None, "`sf` states no level"
    assert staff.velocity_for(None, 1.0) == DYNAMIC_VELOCITY["mf"]
    assert dropped == {"dynamic marking(s) outside ppp..fff": 1}


# ── Reporting ────────────────────────────────────────────────────────────────


def test_the_report_counts_match_the_emitted_notes():
    chart, report = convert("dynamics_levels")
    assert report.velocities.explicit == len(chart.notes)
    assert report.velocities.default == 0
    assert "8 of 8 note(s) explicit" in " | ".join(report.lines())


def test_the_report_counts_notes_left_to_the_default():
    chart, report = convert("single_staff_scale")
    assert (report.velocities.explicit, report.velocities.default) == (0, len(chart.notes))
    assert "0 of 8 note(s) explicit" in " | ".join(report.lines())


def test_no_dynamics_reports_nothing_about_velocity():
    """The pass did not run, so it has nothing to say — same posture as the
    confidence summary on the non-OMR path."""
    _, report = convert("single_staff_scale", read_dynamics=False)
    assert report.velocities is None
    assert not any("velocity" in line for line in report.lines())


# ── CLI ──────────────────────────────────────────────────────────────────────


def run_cli(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, os.path.join(ROOT, "convert.py"), *args],
        capture_output=True,
        text=True,
        check=False,
    )


def test_cli_emits_velocities_and_reports_the_counts_on_stderr():
    result = run_cli("--in", fixture("dynamics_levels"), "--out", "-")
    assert result.returncode == 0
    payload = json.loads(result.stdout)
    assert [n["velocity"] for n in payload["notes"]] == [49] * 4 + [112] * 4
    assert "8 of 8 note(s) explicit" in result.stderr


def test_cli_no_dynamics_matches_an_unmarked_score_byte_for_byte():
    """`--no-dynamics` is the A/B escape hatch: it must produce exactly the
    output this converter produced before the pass existed."""
    marked = run_cli("--in", fixture("dynamics_levels"), "--out", "-", "--no-dynamics")
    assert marked.returncode == 0
    assert all("velocity" not in note for note in json.loads(marked.stdout)["notes"])

    unmarked = run_cli("--in", fixture("single_staff_scale"), "--out", "-")
    flagged = run_cli("--in", fixture("single_staff_scale"), "--out", "-", "--no-dynamics")
    assert unmarked.stdout == flagged.stdout
