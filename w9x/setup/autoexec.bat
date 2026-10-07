@echo off
rem Run Windows 98 Setup with C:\WIN98\MSBATCH.INF until it has installed Windows, then boot Windows.
IF EXIST C:\WINDOWS\WIN.COM GOTO WIN
echo === w98setup start > COM1
C:\WIN98\SMARTDRV.EXE
cd \WIN98
SETUP.EXE C:\WIN98\MSBATCH.INF /IS /IE /NM
echo === w98setup returned > COM1
GOTO END
:WIN
echo === w98 boot windows > COM1
:END
