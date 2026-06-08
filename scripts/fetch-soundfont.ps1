<#
.SYNOPSIS
  Download a piano SoundFont for RockCraft's synth.

.DESCRIPTION
  The synth loads a `.sf2` at runtime (default: crates/audio/assets/piano.sf2),
  which is NOT committed (SoundFonts are large binaries with their own licenses;
  see crates/audio/assets/NOTICE.md). This helper fetches a redistributable
  General-MIDI SoundFont (GeneralUser GS — program 0 is a velocity-layered
  acoustic grand) and drops it at the default path so audio just works.

  Run from the repo root. Skips the download if the file already exists unless
  -Force is given. Point the app at any other .sf2 instead by setting
  ROCKCRAFT_SF2.

.PARAMETER Dest
  Output path. Default: crates/audio/assets/piano.sf2 (the app's default).

.PARAMETER Url
  SoundFont URL. Default: GeneralUser GS (redistributable license).

.PARAMETER Force
  Overwrite an existing file.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\fetch-soundfont.ps1

.EXAMPLE
  # WSL/Linux equivalent (no PowerShell needed):
  #   curl -L --fail -o crates/audio/assets/piano.sf2 \
  #     https://github.com/mrbumpy409/GeneralUser-GS/raw/main/GeneralUser-GS.sf2
#>
param(
    [string]$Dest  = "crates/audio/assets/piano.sf2",
    [string]$Url   = "https://github.com/mrbumpy409/GeneralUser-GS/raw/main/GeneralUser-GS.sf2",
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

if ((Test-Path $Dest) -and -not $Force) {
    Write-Host "SoundFont already present at $Dest (use -Force to re-download)."
    exit 0
}

$destDir = Split-Path -Parent $Dest
if ($destDir -and -not (Test-Path $destDir)) {
    New-Item -ItemType Directory -Path $destDir -Force | Out-Null
}

# Download to a temp file first so a failed/partial download never replaces a
# good file.
$tmp = [System.IO.Path]::GetTempFileName()
try {
    Write-Host "Downloading SoundFont`n  from $Url`n  to   $Dest"
    $ProgressPreference = 'SilentlyContinue'   # much faster IWR without the bar
    Invoke-WebRequest -Uri $Url -OutFile $tmp -MaximumRedirection 5

    # Validate it's actually a SoundFont: RIFF .... sfbk.
    $head = [System.IO.File]::ReadAllBytes($tmp)[0..11]
    $tag0 = [System.Text.Encoding]::ASCII.GetString($head[0..3])
    $tag1 = [System.Text.Encoding]::ASCII.GetString($head[8..11])
    if ($tag0 -ne 'RIFF' -or $tag1 -ne 'sfbk') {
        throw "Downloaded file is not a SoundFont (magic '$tag0'/'$tag1', expected 'RIFF'/'sfbk')."
    }

    Move-Item -Force $tmp $Dest
    $sizeMb = [math]::Round((Get-Item $Dest).Length / 1MB, 1)
    Write-Host "Done: $Dest ($sizeMb MB). It is gitignored (*.sf2) and won't be committed."
}
finally {
    if (Test-Path $tmp) { Remove-Item -Force $tmp }
}
