@echo off
cd /d "%~dp0"
title MailStore Backup Studio

echo ====================================================
echo         Iniciando MailStore Backup Studio
echo ====================================================
echo.

python main.py
if %ERRORLEVEL% EQU 0 goto :fim

echo.
echo Tentando iniciar com o executavel 'py'...
py main.py
if %ERRORLEVEL% EQU 0 goto :fim

echo.
echo [ERRO] Nao foi possivel iniciar a aplicacao.
echo Verifique se o Python esta instalado e adicionado ao PATH.
echo.
pause

:fim
