@echo off
REM Run the Tauri desktop app (Windows). Run from a normal cmd prompt.
REM
REM   run-tauri.bat            run the optimised (release) build - the default
REM   run-tauri.bat --debug    run the debug build (from build-tauri.bat --debug)
REM
REM `--release` is still accepted and means the default.
REM
REM Anything after those flags is passed straight to the app.
REM
REM The control socket defaults on at 127.0.0.1:9001 so an agent (or
REM scripts\local\rc.mjs) can drive the app. Set ROCKCRAFT_CONTROL_ADDR yourself
REM to move it, or NO_CONTROL=1 to start without it.
setlocal enabledelayedexpansion
cd /d "%~dp0"

set "PROFILE=release"
set "APP_ARGS="
REM Collect the args with shift, not `for %%a in (%*)`: a for block is
REM expanded once, so accumulating into a variable there keeps only the last
REM argument.
:args
if "%~1"=="" goto args_done
if "%~1"=="--release" (
  set "PROFILE=release"
) else if "%~1"=="--debug" (
  set "PROFILE=debug"
) else (
  set "APP_ARGS=!APP_ARGS! %1"
)
shift
goto args
:args_done

set "EXE=target-win\%PROFILE%\rockcraft-tauri.exe"
if not exist "%EXE%" (
  echo error: %EXE% not found - build it first:
  if "%PROFILE%"=="release" (
    echo          build-tauri.bat
  ) else (
    echo          build-tauri.bat --debug
  )
  exit /b 1
)

REM The app is single-instance: a second copy exits immediately. Say so plainly
REM rather than letting it look like a silent failure.
tasklist /FI "IMAGENAME eq rockcraft-tauri.exe" 2>nul | find /I "rockcraft-tauri.exe" >nul
if not errorlevel 1 (
  echo note: RockCraft is already running - not starting a second copy.
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
    echo note: no SoundFont at crates\audio\assets\piano.sf2 - audio will be silent.
  )
)

echo Starting %EXE%
if not "%ROCKCRAFT_CONTROL_ADDR%"=="" echo   control socket: ws://%ROCKCRAFT_CONTROL_ADDR%
"%EXE%" !APP_ARGS!
endlocal
