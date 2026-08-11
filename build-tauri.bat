@echo off
REM Build the Tauri desktop app (Windows). Run from a normal cmd prompt.
REM
REM   build-tauri.bat            build
REM   build-tauri.bat --release  optimised build
REM
REM Output: target-win\<profile>\rockcraft-tauri.exe  (run it with run-tauri.bat)
REM
REM Two things this exists to get right:
REM
REM  1. It is a TWO-step build. `custom-protocol` embeds the frontend into the
REM     exe at *compile* time, so a frontend-only change needs `vite build` AND a
REM     Rust recompile. Cargo will not redo the embed unless a .rs file changed,
REM     so we bump lib.rs's timestamp — without that the exe silently keeps the
REM     OLD frontend, which looks exactly like your change not working.
REM  2. Without --features tauri/custom-protocol the webview loads from a dev
REM     server that isn't running, and the window shows "localhost refused to
REM     connect".
setlocal
cd /d "%~dp0"

set "PROFILE=debug"
set "CARGO_ARGS="
for %%a in (%*) do (
  if "%%a"=="--release" (
    set "PROFILE=release"
    set "CARGO_ARGS=!CARGO_ARGS! --release"
  ) else (
    set "CARGO_ARGS=!CARGO_ARGS! %%a"
  )
)
setlocal enabledelayedexpansion

where cargo >nul 2>&1
if errorlevel 1 (
  echo error: cargo not found on PATH. Install Rust for Windows: https://rustup.rs
  exit /b 1
)

echo [1/3] frontend ^(vite build^)
pushd tauri-app
call npx vite build
if errorlevel 1 ( popd & echo error: frontend build failed & exit /b 1 )
popd

REM Bump the timestamp so cargo re-embeds the frontend (see the header).
echo [2/3] touch lib.rs so the frontend is re-embedded
copy /b "tauri-app\src-tauri\src\lib.rs" +,, >nul

echo [3/3] Windows binary ^(%PROFILE%^)
cargo build -p rockcraft-tauri --features tauri/custom-protocol --target-dir target-win %CARGO_ARGS%
if errorlevel 1 (
  echo error: cargo build failed.
  echo        If it says the exe is locked, close RockCraft first.
  exit /b 1
)

echo.
echo Built target-win\%PROFILE%\rockcraft-tauri.exe
echo Run it with:  run-tauri.bat
endlocal
