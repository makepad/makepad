@echo off
setlocal

set "SOURCE=%~dp0"
if "%SOURCE:~-1%"=="\" set "SOURCE=%SOURCE:~0,-1%"

"%SOURCE%\clang.exe" ^
  -target aarch64-linux-ohos ^
  --sysroot="%SOURCE%\..\..\sysroot" ^
  -D__MUSL__ ^
  %*

endlocal
exit /b
