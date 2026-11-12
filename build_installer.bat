@echo off
REM Builds the program and the Setup installer.
REM Needs: Rust (https://rustup.rs) and Inno Setup 6 (https://jrsoftware.org/isdl.php)
cargo build --release || goto :err
if not exist dist mkdir dist
set ISCC=
if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe
if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe
if "%ISCC%"=="" (
  echo Inno Setup 6 not found. Install it from https://jrsoftware.org/isdl.php and run again.
  pause & exit /b 1
)
"%ISCC%" installer\RiponAIPhotoStudio.iss || goto :err
echo.
echo DONE  ->  dist\rust.riponaistudio-Setup-1.4.3.exe
pause
exit /b 0
:err
echo Build failed.
pause
