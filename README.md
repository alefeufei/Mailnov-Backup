# Mailnov Backup

<p align="center">
  <img src="mailstore/web/favicon.png" alt="Mailnov Backup Logo" width="96" height="96">
</p>

Sistema profissional, completo e moderno para **backup, arquivamento incremental, pesquisa instantânea e exportação/migração de e-mails IMAP**, inspirado na experiência do MailStore Home.

Permite arquivar caixas postais completas de qualquer provedor IMAP (Gmail, Outlook/Office 365, Yahoo, iCloud, Fastmail, cPanel, Dovecot, Zimbra, Exchange e servidores dedicados), pesquisar instantaneamente mensagens com anexos, e exportar ou migrar tudo de volta para outro servidor IMAP ou diretamente para arquivos **PST do Microsoft Outlook (.pst)**, diretórios de **.EML** ou arquivos **.MBOX**.

---

## 🚀 Principais Funcionalidades

### 1. Arquivamento IMAP Inteligente & Incremental
- **Conexão Resiliente**: Suporte total a SSL/TLS (porta 993), STARTTLS (porta 143/587) e conexões personalizadas.
- **Detecção Automática de Provedor**: Preenchimento automático de host, porta e criptografia para Gmail, Microsoft 365, Yahoo, iCloud, Fastmail, Zoho e cPanel.
- **Backup Incremental Sem Duplicatas**: Baixa apenas mensagens novas via delta de UIDs e deduplicação criptográfica por hash SHA-256.
- **Acesso Seguro Somente Leitura (`READ-ONLY`)**: Nunca altera, marca ou apaga e-mails no servidor de origem durante a sincronização.
- **Preservação de Pastas**: Mantém a hierarquia de pastas original (Caixa de Entrada, Enviados, Rascunhos, Lixeira e pastas customizadas com codificação IMAP UTF-7).
- **Filtros Flexíveis**: Escolha pastas específicas e filtre mensagens a partir de determinada data.

### 2. Exportação e Migração com Autocura (Dovecot / cPanel / Gmail)
- **Migração para Outro Servidor IMAP**:
  - Transfere e-mails em lote com comando `APPEND`.
  - Recria toda a árvore de pastas no destino automaticamente.
  - **Autocura de Namespace Dovecot**: detecta e ajusta automaticamente hierarquias com prefixo `INBOX.` (ex: `INBOX.Sent`, `INBOX.Trash`, `INBOX.Restaurado`).
  - **Preserva Data Interna Original (`INTERNALDATE`)**: as mensagens chegam no destino com a data e hora exatas em que foram recebidas.
  - **Preserva Flags**: mantém status de lido (`\Seen`), respondido (`\Answered`), sinalizado (`\Flagged`), etc.
  - **Fallback de Segurança**: garante que nenhuma mensagem seja perdida caso uma pasta específica seja restrita pelo servidor.
- **Exportação para Arquivo PST do Microsoft Outlook (.pst)**:
  - Cria arquivos `.pst` nativos compatíveis com todas as versões do Microsoft Outlook.
  - Botão interativo **Localizar / Escolher Pasta** que abre a caixa nativa do Windows (*Salvar Como*).
- **Exportação para Diretório de Arquivos .EML**:
  - Salva cópias dos e-mails individuais no formato padrão RFC 822 estruturados em pastas.
- **Exportação para Arquivo .MBOX**:
  - Gera arquivo padrão RFC 4155 compatível com Mozilla Thunderbird e Apple Mail.

### 3. Navegação por Conta & Visualizador Moderno
- **Árvore de Pastas Isolada por Perfil**: navegue no acervo de cada conta de forma independente, sem misturar remetentes ou históricos.
- **Motor de Busca em Texto Completo (FTS5)**: Pesquisa instantânea em milhões de e-mails por assunto, remetente, destinatário e corpo.
- **Visualizador Seguro de E-mails**:
  - Cabeçalhos detalhados (De, Para, Cc, Assunto, Data).
  - Alternância entre visualização HTML formatada (em sandbox seguro) e Texto Puro.
  - Lista de anexos com tamanho e download direto.
  - Botão para baixar o arquivo `.eml` original.

### 4. Interface Desktop Moderna
- Modais translúcidos com efeito *glassmorphism* e cantos arredondados.
- Cartões de estatísticas dinâmicas em tempo real (Total, Sucessos, Falhas).
- Console/Terminal em tempo real com acompanhamento detalhado da operação.
- Favicon e identidade visual exclusiva.

---

## 📂 Estrutura do Projeto

```
mailbackup/
├── mailstore/
│   ├── config.py         # Configurações de diretórios e provedores
│   ├── models.py         # Modelos Pydantic de dados e requisições
│   ├── database.py       # Banco SQLite com FTS5 e controle de transações
│   ├── parser.py         # Extrator RFC 822 / MIME, anexos e SHA-256
│   ├── imap_engine.py    # Cliente IMAP incremental resiliente
│   ├── pst_engine.py     # Gerador nativo de PST via automação MAPI do Outlook
│   ├── export_engine.py  # Exportação para IMAP (APPEND), EML e MBOX com autocura
│   ├── verifier.py       # Auditor de integridade criptográfica SHA-256
│   ├── server.py         # Servidor REST API e servidor web embutido (FastAPI)
│   └── web/              # Interface Web Desktop (HTML5, CSS3, JS Puro)
│       ├── index.html
│       ├── favicon.svg
│       ├── favicon.ico
│       ├── favicon.png
│       ├── css/app.css
│       └── js/app.js
├── cli.py                # Interface de linha de comando completa
├── main.py               # Ponto de entrada que inicia o app e abre o navegador
├── iniciar.bat           # Executável com 2 cliques para Windows
├── backup_agendado.bat   # Script pronto para o Agendador de Tarefas do Windows
├── requirements.txt      # Dependências Python
└── tests/                # Suíte de testes automatizados unitários e e2e
```

---

## 🛠️ Instalação Local (Passo a Passo)

### 1. Pré-requisitos
- **Python**: versão 3.10 ou superior ([Download Python](https://www.python.org/downloads/)). *Marque a opção "Add Python to PATH" durante a instalação no Windows.*
- **Microsoft Outlook** (opcional): obrigatório apenas para exportação nativa em arquivo `.pst` no Windows. Para arquivamento IMAP, exportação IMAP, EML e MBOX, o Outlook **não** é necessário.

### 2. Clonar o Repositório
```bash
git clone https://github.com/seu-usuario/mailbackup.git
cd mailbackup
```

### 3. Criar e Ativar o Ambiente Virtual (Recomendado)
- **Windows (Prompt / PowerShell)**:
  ```powershell
  python -m venv venv
  .\venv\Scripts\activate
  ```
- **Linux / macOS**:
  ```bash
  python3 -m venv venv
  source venv/bin/activate
  ```

### 4. Instalar as Dependências
```bash
pip install -r requirements.txt
```

### 5. Executar a Aplicação
- **No Windows**: basta dar um duplo clique em **`iniciar.bat`** ou executar:
  ```bash
  python main.py
  ```
- O servidor iniciará automaticamente na porta `8000` (ou na próxima porta livre, como `8001`) e a interface gráfica abrirá de imediato no seu navegador padrão em:
  ```
  http://127.0.0.1:8000
  ```

---

## 🌐 Guia de Deploy em Produção / Servidor

O Mailnov Backup pode ser implantado em servidores dedicados ou VPS (Windows Server ou Linux) para operar continuamente em rede ou executar rotinas automáticas de backup.

### Variável de Ambiente para Armazenamento Externo
Por padrão, os dados são armazenados na pasta `./archive_data` dentro do projeto. Para apontar para outro disco, storage corporativo ou volume persistente:

- **Windows**:
  ```cmd
  setx MAILBACKUP_DATA_DIR "D:\MailnovStorage"
  ```
- **Linux**:
  ```bash
  export MAILBACKUP_DATA_DIR="/mnt/storage/mailnov"
  ```

---

### Opção A: Deploy no Windows Server como Serviço (NSSM)

Para que a aplicação rode como um serviço do Windows em segundo plano, mesmo sem nenhum usuário conectado:

1. Baixe o [NSSM (Non-Sucking Service Manager)](https://nssm.cc/download).
2. Abra o Prompt de Comando (CMD) como Administrador e execute:
   ```cmd
   nssm install MailnovBackup
   ```
3. Preencha os campos na janela do NSSM:
   - **Path**: `C:\caminho\para\mailbackup\venv\Scripts\python.exe`
   - **Startup directory**: `C:\caminho\para\mailbackup`
   - **Arguments**: `main.py`
4. Na aba **Details**, defina o Display Name como `Mailnov Backup Studio`.
5. Clique em **Install service** e inicie o serviço:
   ```cmd
   nssm start MailnovBackup
   ```

---

### Opção B: Deploy no Linux (Ubuntu / Debian / CentOS)

O motor IMAP, o banco SQLite FTS5 e as exportações IMAP, EML e MBOX rodam perfeitamente no Linux.

#### 1. Configurar o Serviço no `systemd`
Crie o arquivo `/etc/systemd/system/mailnov.service`:
```ini
[Unit]
Description=Mailnov Backup Service
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=/var/www/mailbackup
Environment="MAILBACKUP_DATA_DIR=/var/mailnov_data"
ExecStart=/var/www/mailbackup/venv/bin/uvicorn mailstore.server:app --host 127.0.0.1 --port 8000 --workers 2
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Ative e inicie o serviço:
```bash
sudo systemctl daemon-reload
sudo systemctl enable mailnov
sudo systemctl start mailnov
```

#### 2. Configurar Proxy Reverso com Nginx e SSL (HTTPS)
Crie o arquivo `/etc/nginx/sites-available/mailnov`:
```nginx
server {
    listen 80;
    server_name mailbackup.suaempresa.com.br;

    # Redireciona HTTP para HTTPS
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name mailbackup.suaempresa.com.br;

    ssl_certificate /etc/letsencrypt/live/mailbackup.suaempresa.com.br/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/mailbackup.suaempresa.com.br/privkey.pem;

    client_max_body_size 100M;

    # Opcional: Autenticação básica de acesso
    # auth_basic "Área Restrita Mailnov Backup";
    # auth_basic_user_file /etc/nginx/.htpasswd;

    location / {
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Suporte a SSE / Streaming
        proxy_buffering off;
        proxy_read_timeout 300s;
    }
}
```
Ative o site e recarregue o Nginx:
```bash
sudo ln -s /etc/nginx/sites-available/mailnov /etc/nginx/sites-enabled/
sudo nginx -t && sudo systemctl reload nginx
```

---

## 💻 Uso da Linha de Comando (CLI)

O Mailnov Backup possui uma interface de linha de comando completa (`cli.py`) para automações via scripts e servidores sem interface gráfica:

### Gerenciar Perfis
```bash
# Listar perfis cadastrados
python cli.py profiles

# Criar um novo perfil IMAP
python cli.py add-profile --name "Corporativo Inovatti" --host "mail.inovatti.com.br" --port 993 --ssl "SSL" --user "alexandre@inovatti.com.br" --password "senha123"
```

### Executar Arquivamento
```bash
# Arquivar por ID ou Nome do perfil
python cli.py backup --profile "Corporativo Inovatti"
```

### Exportar / Migrar para Outro Servidor IMAP
```bash
python cli.py export-imap --profile "Corporativo Inovatti" --target-host "imap.novoprovedor.com" --target-user "novo@novoprovedor.com" --target-pass "senha123" --prefix "Restaurado"
```

### Exportar para Arquivo PST do Outlook
```bash
python cli.py export-pst --profile "Corporativo Inovatti" --output "C:\Backups\Outlook.pst"
```

### Exportar para Diretório EML ou Arquivo MBOX
```bash
# EML
python cli.py export-eml --profile "Corporativo Inovatti" --output-dir "C:\Backups\EML"

# MBOX
python cli.py export-mbox --profile "Corporativo Inovatti" --output "C:\Backups\emails.mbox"
```

### Pesquisar no Arquivo e Auditoria
```bash
# Pesquisa rápida no índice FTS5
python cli.py search "fatura ou boleto"

# Verificação criptográfica de integridade SHA-256
python cli.py verify
```

---

## ⏰ Automação no Windows (Agendador de Tarefas)

Para rodar backups automáticos todas as noites:
1. Abra o **Agendador de Tarefas do Windows** (`taskschd.msc`).
2. Clique em **Criar Tarefa Básica**.
3. Defina o agendamento (ex: Diariamente às 23:00).
4. Na ação, selecione **Iniciar um programa**:
   - **Programa/script**: `C:\Github\mailbackup\backup_agendado.bat`
   - **Adicionar argumentos**: `"NomeDoPerfil"`
   - **Iniciar em**: `C:\Github\mailbackup`

---

## 🧪 Execução dos Testes Automatizados

A suíte de testes cobre todas as funcionalidades do motor, conexões IMAP simuladas, parsing MIME, banco de dados e APIs:

```bash
python -m unittest discover tests
```

---

## 🔒 Segurança & Privacidade

- Todas as senhas de conexão são armazenadas localmente no banco de dados da máquina do usuário.
- O modo `READ-ONLY` previne qualquer modificação acidental nas caixas postais do servidor de origem.
- O visualizador de e-mails executa o conteúdo HTML dentro de um sandbox isolado para prevenir execução de scripts maliciosos.

---

## 📄 Licença

Desenvolvido para segurança, preservação e migração autônoma de dados de e-mail.
