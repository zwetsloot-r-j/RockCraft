@echo off
REM Build the terminal (TUI) app (Windows). Run from a normal cmd prompt.
REM
REM   build-tui.bat            build
REM   build-tui.bat --release  optimised build
REM
REM Anything else is passed straight to cargo.
REM
REM Output: target-win\<profile>\rockcraft.exe  (run it with run-tui.bat)
REM
REM Builds into target-win\ (not target\) so a Windows build never clobbers the
REM Linux/WSL one, and vice versa.
setlocal enabledelayedexpansion
cd /d "%~dp0"

set "PROFILE=debug"
set "CARGO_ARGS="
:args
if "%~1"=="" goto args_done
if "%~1"=="--release" set "PROFILE=release"
set "CARGO_ARGS=!CARGO_ARGS! %1"
shift
goto args
:args_done

where cargo >nul 2>&1
if errorlevel 1 (
  echo error: cargo not found on PATH. Install Rust for Windows: https://rustup.rs
  exit /b 1
)

echo Building the TUI ^(%PROFILE%^)
cargo build -p rockcraft-tui --target-dir target-win !CARGO_ARGS!
if errorlevel 1 (
  echo error: cargo build failed.
  echo        If it says the exe is locked, close RockCraft first.
  exit /b 1
)

echo.
echo Built target-win\%PROFILE%\rockcraft.exe
echo Run it with:  run-tui.bat
endlocal
