# tools/tempo-map — tempo map from audio (M16-A)

Detects a **per-bar tempo map** from a piece's backing audio: the file time of
every bar's downbeat, following a performance whose tempo breathes (rubato, a
hand-played cover). numpy + the standard library only.

```
python3 tempo_map.py --in backing.wav --out - [--beats-per-bar 4]
                     [--anchor-us 2536000] [--tempo-hint 86]
```

- `--anchor-us` — a known downbeat (file µs); snapped to the nearest beat.
  Without it the bar phase is picked from bass accents.
- `--tempo-hint` — the expected beat BPM; resolves half/double time.

Output: `{"version":1, "beats_us":[…], "bars_us":[…], "bpm":…, "pulse_bpm":…,
"anchor_us":…}` — all file times. The last bar entry closes the final bar.

Method: spectral-flux onset envelope → autocorrelation tempo with a log-Gaussian
prior → Ellis (2007) dynamic-programming beat tracker (librosa's recipe) → fold a
fast accented eighth pulse to quarter beats (re-tracking at the doubled period)
→ group beats into bars from the anchor.

The app runs it through the `detect_tempo_map` host command (key `I` in the
Tauri edit screen), which converts to song time and installs the map with the
`set_bar_starts` action. Tests (`python3 -m pytest`) use synthetic click tracks
only — never commit real audio.
