"""Configurações e caminhos padrão do MailStore Backup."""

import os
import sys
from pathlib import Path

# Diretório base de armazenamento de dados
# Prioridade: 1) Variável de ambiente MAILBACKUP_DATA_DIR; 2) ./archive_data local
DEFAULT_DATA_DIR = os.environ.get(
    "MAILBACKUP_DATA_DIR",
    str(Path(__file__).resolve().parent.parent / "archive_data")
)

BASE_DIR = Path(DEFAULT_DATA_DIR).resolve()
DB_PATH = BASE_DIR / "mailstore.db"
STORAGE_DIR = BASE_DIR / "eml_storage"
EXPORTS_DIR = BASE_DIR / "exports"
LOGS_DIR = BASE_DIR / "logs"

# Criação inicial de diretórios essenciais
for folder in [BASE_DIR, STORAGE_DIR, EXPORTS_DIR, LOGS_DIR]:
    folder.mkdir(parents=True, exist_ok=True)

# Provedores pré-configurados para detecção e facilidade de uso
KNOWN_PROVIDERS = {
    "gmail": {
        "name": "Gmail / Google Workspace",
        "domains": ["gmail.com", "googlemail.com"],
        "host": "imap.gmail.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Requer 'Senha de Aplicativo' se 2FA estiver ativado."
    },
    "outlook": {
        "name": "Outlook.com / Hotmail / Office 365",
        "domains": ["outlook.com", "hotmail.com", "live.com", "msn.com", "office365.com"],
        "host": "outlook.office365.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Utilize Senha de Aplicativo ou credenciais da conta Microsoft."
    },
    "yahoo": {
        "name": "Yahoo Mail",
        "domains": ["yahoo.com", "yahoo.com.br", "ymail.com"],
        "host": "imap.mail.yahoo.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Gere uma Senha de Aplicativo na segurança da sua conta Yahoo."
    },
    "icloud": {
        "name": "Apple iCloud Mail",
        "domains": ["icloud.com", "me.com", "mac.com"],
        "host": "imap.mail.me.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Requer Senha de Aplicativo gerada no appleid.apple.com."
    },
    "zoho": {
        "name": "Zoho Mail",
        "domains": ["zoho.com"],
        "host": "imappro.zoho.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Habilite acesso IMAP nas configurações do Zoho Mail."
    },
    "fastmail": {
        "name": "Fastmail",
        "domains": ["fastmail.com", "fastmail.fm"],
        "host": "imap.fastmail.com",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Gere uma App Password nas configurações de segurança."
    },
    "cpanel": {
        "name": "cPanel / Hospedagem Padrão",
        "domains": [],
        "host": "mail.dominio.com.br",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Substitua pelo host do seu servidor de hospedagem."
    }
}


def detect_provider(email_address: str):
    """Detecta automaticamente os parâmetros do servidor a partir do e-mail."""
    if not email_address or "@" not in email_address:
        return None
    domain = email_address.strip().split("@")[-1].lower()
    for key, data in KNOWN_PROVIDERS.items():
        if domain in data["domains"]:
            return {
                "key": key,
                "name": data["name"],
                "host": data["host"],
                "port": data["port"],
                "ssl_mode": data["ssl_mode"],
                "notes": data["notes"]
            }
    # Tenta padrão corporativo mail.<dominio>
    return {
        "key": "custom",
        "name": f"Servidor {domain}",
        "host": f"mail.{domain}",
        "port": 993,
        "ssl_mode": "SSL",
        "notes": "Configuração detectada automaticamente para o domínio."
    }
