@echo off
chcp 65001 >nul
REM Script para execução via Agendador de Tarefas do Windows (Task Scheduler)
REM Uso: backup_agendado.bat "NomeDoPerfilOuID"

if "%~1"=="" (
    echo [ERRO] Informe o nome ou ID do perfil de backup.
    echo Exemplo de uso: backup_agendado.bat "Gmail Pessoal"
    exit /b 1
)

echo [%date% %time%] Iniciando backup agendado do perfil: %~1
python "%~dp0cli.py" backup --profile "%~1"
echo [%date% %time%] Backup agendado finalizado com codigo %ERRORLEVEL%
