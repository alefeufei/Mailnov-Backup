"""Ponto de entrada principal do MailStore Backup Studio."""

import sys
import webbrowser
import uvicorn
from mailstore.config import BASE_DIR
from cli import main as cli_main


def run_app():
    if len(sys.argv) > 1:
        cli_main()
    else:
        port = 8000
        url = f"http://127.0.0.1:{port}"
        print("=" * 60)
        print(" MailStore Backup Studio - Inicializando...")
        print(f" Interface Web: {url}")
        print(f" Repositório Local: {BASE_DIR}")
        print("=" * 60)
        
        # Abre o navegador automaticamente
        try:
            webbrowser.open(url)
        except Exception:
            pass

        uvicorn.run("mailstore.server:app", host="127.0.0.1", port=port, log_level="info")


if __name__ == "__main__":
    run_app()
