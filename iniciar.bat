@echo off
chcp 65001 >nul
title MailStore Backup Studio
echo ====================================================
echo         Iniciando MailStore Backup Studio
echo ====================================================
echo.
python main.py
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo Ocorreu um erro ao iniciar a aplicação.
    pause
)
