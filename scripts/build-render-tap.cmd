@echo off
setlocal
call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
if errorlevel 1 exit /b 1
cd /d "%~dp0.."
if not exist target\render-tap mkdir target\render-tap
if /i "%~1"=="collector" goto collector
cl /nologo /std:c++17 /O2 /EHsc /MT /W4 /LD /I target\detours\include apps\render-tap\tap.cpp /Fo:target\render-tap\tap.obj /link /OUT:target\render-tap\render-tap.dll /IMPLIB:target\render-tap\tap.lib target\detours\lib.X64\detours.lib ole32.lib uuid.lib
if errorlevel 1 exit /b 1
:collector
cl /nologo /std:c++17 /O2 /EHsc /MT /W4 apps\render-tap\probe.cpp /Fo:target\render-tap\probe.obj /link /OUT:target\render-tap\render-probe.exe ole32.lib uuid.lib avrt.lib
exit /b %errorlevel%
