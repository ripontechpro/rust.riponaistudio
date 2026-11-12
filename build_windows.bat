@echo off
REM Build Ripon AI Photo Studio on Windows 10/11.
REM 1) Install Rust once from https://rustup.rs  (choose the default "MSVC" option; it asks to install
REM    "Visual Studio C++ Build Tools" - say yes)
REM 2) Double-click this file.  Result:  dist\Ripon AI Photo Studio.exe
cargo build --release || goto :err
if not exist dist mkdir dist
if not exist dist\dress mkdir dist\dress
copy /Y target\release\ripon-ai-photo-studio.exe "dist\Ripon AI Photo Studio.exe" >nul
copy /Y dress\README.txt dist\dress\ >nul
echo.
echo DONE  ->  dist\Ripon AI Photo Studio.exe
pause
exit /b 0
:err
echo Build failed. Is Rust installed?  https://rustup.rs
pause
