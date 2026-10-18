@echo off
rem Run icrs on Windows. Double-click it, or call it with arguments:
rem
rem   icrs.bat
rem   icrs.bat info
rem   icrs.bat watch zone gems --interval 500
rem
rem Finds an existing exe before building one, so this also works dropped next
rem to a downloaded icrs.exe. It pauses at the end so a double-clicked window
rem stays readable; set ICRS_NO_PAUSE=1 to turn that off when scripting.

setlocal
set "HERE=%~dp0"
set "TRIPLE=x86_64-pc-windows-gnu"
set "EXE="

if exist "%HERE%icrs.exe" set "EXE=%HERE%icrs.exe"
if not defined EXE if exist "%HERE%target\%TRIPLE%\release\icrs.exe" set "EXE=%HERE%target\%TRIPLE%\release\icrs.exe"
if not defined EXE if exist "%HERE%target\%TRIPLE%\debug\icrs.exe" set "EXE=%HERE%target\%TRIPLE%\debug\icrs.exe"

if not defined EXE (
    where cargo >/dev/null 2>&1
    if errorlevel 1 (
        echo No icrs.exe here, and cargo is not installed.
        echo Put icrs.exe next to this file, or install Rust and run again.
        goto :fail
    )
    echo Building icrs.exe ...
    pushd "%HERE%"
    cargo build --release
    if errorlevel 1 (
        popd
        echo Build failed.
        goto :fail
    )
    popd
    set "EXE=%HERE%target\%TRIPLE%\release\icrs.exe"
)

if not exist "%EXE%" (
    echo Could not find "%EXE%".
    goto :fail
)

"%EXE%" %*
set "CODE=%ERRORLEVEL%"
if not defined ICRS_NO_PAUSE pause
exit /b %CODE%

:fail
if not defined ICRS_NO_PAUSE pause
exit /b 1
