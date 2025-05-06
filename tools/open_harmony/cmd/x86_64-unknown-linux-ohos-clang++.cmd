@echo on
setlocal

set "SOURCE=%~dp0"
if "%SOURCE:~-1%"=="\" set "SOURCE=%SOURCE:~0,-1%"

"%SOURCE%\clang++.exe" ^
  -target x86_64-linux-ohos ^
  --sysroot="%SOURCE%\..\..\sysroot" ^
  -D__clang__ ^
  -D__MUSL__ ^
  %*

endlocal
exit /b
