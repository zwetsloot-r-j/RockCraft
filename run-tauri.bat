@echo off
REM Run the Tauri desktop app (Windows). Run from a normal cmd prompt.
REM
REM   run-tauri.bat            run
REM   run-tauri.bat --release  run the optimised build
REM
REM Anything after those flags is passed straight to the app.
REM
REM The control socket defaults on at 127.0.0.1:9001 so an agent (or
REM scripts\local\rc.mjs) can drive the app. Set ROCKCRAFT_CONTROL_ADDR yourself
REM to move it, or NO_CONTROL=1 to start without it.
setlocal
cd /d "%~dp0"

set "PROFILE=debug"
set "APP_ARGS="
for %%a in (%*) do (
  if "%%a"=="--release" (
    set "PROFILE=release"
  ) else (
    set "APP_ARGS=%APP_ARGS% %%a"
  )
)

set "EXE=target-win\%PROFILE%\rockcraft-tauri.exe"
if not exist "%EXE%" (
  echo error: %EXE% not found — build it first:
  echo          build-tauri.bat
  exit /b 1
)

REM The app is single-instance: a second copy exits immediately. Say so plainly
REM rather than letting it look like a silent failure.
tasklist /FI "IMAGENAME eq rockcraft-tauri.exe" 2>nul | find /I "rockcraft-tauri.exe" >nul
if not errorlevel 1 (
  echo note: RockCraft is already running — not starting a second copy.
  exit /b 0
)

if not "%NO_CONTROL%"=="1" (
  if "%ROCKCRAFT_CONTROL_ADDR%"=="" set "ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001"
)

REM Audio needs a SoundFont: without one the whole audio thread dies, taking the
REM backing track with it, not just the synth.
if "%ROCKCRAFT_SF2%"=="" (
  if exist "crates\audio\assets\piano.sf2" (
    set "ROCKCRAFT_SF2=%CD%\crates\audio\assets\piano.sf2"
  ) else (
    echo note: no SoundFont at crates\audio\assets\piano.sf2 — audio will be silent.
  )
)

echo Starting %EXE%
if not "%ROCKCRAFT_CONTROL_ADDR%"=="" echo   control socket: ws://%ROCKCRAFT_CONTROL_ADDR%
"%EXE%" %APP_ARGS%
endlocal
