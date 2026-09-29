@echo off
REM Run the terminal (TUI) app (Windows), in this console. Run from a normal
REM cmd prompt or Windows Terminal (it needs full RGB colour for the highway).
REM
REM   run-tui.bat                          piano (port name containing "casio")
REM   run-tui.bat --mock                   no piano: play with the number row 1-0
REM   run-tui.bat --edit                   open straight in the composer
REM   run-tui.bat --control                start the agent control socket
REM   run-tui.bat casio song.wav           piano + a backing track while recording
REM   run-tui.bat --release ...            run the optimised build
REM
REM Everything except --release is passed to the app in order. Order matters:
REM the first non-flag argument is the MIDI port name, and a backing track must
REM be the SECOND argument overall - so don't put a flag before it; to use
REM the control socket with a backing track, set ROCKCRAFT_CONTROL_ADDR instead.
setlocal enabledelayedexpansion
cd /d "%~dp0"

set "PROFILE=debug"
set "APP_ARGS="
:args
if "%~1"=="" goto args_done
if "%~1"=="--release" (
  set "PROFILE=release"
) else (
  set "APP_ARGS=!APP_ARGS! %1"
)
shift
goto args
:args_done

set "EXE=target-win\%PROFILE%\rockcraft.exe"
if not exist "%EXE%" (
  echo error: %EXE% not found - build it first:
  echo          build-tui.bat
  exit /b 1
)

REM Audio needs a SoundFont; without one the app runs silently.
if "%ROCKCRAFT_SF2%"=="" (
  if exist "crates\audio\assets\piano.sf2" (
    set "ROCKCRAFT_SF2=%CD%\crates\audio\assets\piano.sf2"
  ) else (
    echo note: no SoundFont at crates\audio\assets\piano.sf2 - audio will be silent.
  )
)

"%EXE%" !APP_ARGS!
endlocal
