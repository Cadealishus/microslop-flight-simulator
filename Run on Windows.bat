@echo off
rem Microslop Flight Simulator on Windows: builds the game the first time
rem (several minutes), then starts it. Double-click this file.
title Microslop Flight Simulator
cd /d "%~dp0"
if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed yet.
  echo Download and run rustup-init.exe from https://rustup.rs
  echo ^(if it asks for the Visual Studio C++ build tools, let it install them^),
  echo then open this file again.
  echo.
  pause
  exit /b 1
)
echo Starting Microslop Flight Simulator. The first time, building it takes several minutes...
cargo run --release
if errorlevel 1 (
  echo.
  echo The game stopped with an error ^(see above^).
  pause
)
