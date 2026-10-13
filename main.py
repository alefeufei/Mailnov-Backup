"""Ponto de entrada principal do MailStore Backup Studio."""

import socket
import sys
import webbrowser
import uvicorn
from mailstore.config import BASE_DIR
from cli import main as cli_main


def find_free_port(start_port: int = 8000, max_attempts: int = 50) -> int:
    """Encontra uma porta TCP livre para vincular o servidor web."""
    for port in range(start_port, start_port + max_attempts):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    return start_port


def run_app():
    if len(sys.argv) > 1:
        cli_main()
    else:
        port = find_free_port(8000)
        url = f"http://127.0.0.1:{port}"
        print("=" * 60)
        print(" MailStore Backup Studio - Inicializando...")
        print(f" Interface Web: {url}")
        print(f" Repositorio Local: {BASE_DIR}")
        print("=" * 60)
        
        # Abre o navegador automaticamente
        try:
            webbrowser.open(url)
        except Exception:
            pass

        uvicorn.run("mailstore.server:app", host="127.0.0.1", port=port, log_level="info")


if __name__ == "__main__":
    run_app()
