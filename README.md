# Mailnov Backup

Sistema completo e profissional para **backup, arquivamento incremental, pesquisa e exportação/migração de e-mails IMAP**, inspirado na experiência do MailStore Home.

Permite arquivar caixas postais inteiras de qualquer provedor IMAP (Gmail, Outlook/Office 365, Yahoo, iCloud, Fastmail, cPanel, servidores corporativos), pesquisar instantaneamente mensagens com anexos, e exportar tudo de volta para outro servidor IMAP ou diretamente para arquivos **PST do Microsoft Outlook (.pst)**, pastas de **.EML** ou arquivos **.MBOX**.

---

## Principais Funcionalidades

### 1. Arquivamento IMAP Inteligente & Incremental
- **Conexão Resiliente**: Suporte total a SSL/TLS, STARTTLS e modo direto (portas 993, 143, personalizadas).
- **Detecção Automática de Provedor**: Preenchimento automático para Gmail, Microsoft 365, Yahoo, iCloud, Fastmail, Zoho e cPanel ao digitar o e-mail.
- **Backup Incremental Sem Duplicatas**: Faz o download apenas de mensagens novas via delta de UIDs e deduplicação criptográfica por hash SHA-256.
- **Acesso Seguro Somente Leitura (`READ-ONLY`)**: Nunca altera, marca ou apaga mensagens no servidor durante o arquivamento.
- **Preservação de Hierarquia**: Mantém a estrutura exata de pastas (Caixa de Entrada, Enviados, Rascunhos, Lixeira e pastas customizadas com codificação IMAP UTF-7).
- **Filtros Flexíveis**: Escolha pastas específicas e filtre mensagens a partir de determinada data.

### 2. Exportação e Migração Completa
- **Migração / Restauração para outro Servidor IMAP**:
  - Transfere e-mails para uma nova conta via comando IMAP `APPEND`.
  - Recria a árvore de pastas no servidor de destino automaticamente.
  - **Preserva a data interna original (`INTERNALDATE`)**: as mensagens chegam no destino com a data exata em que foram recebidas, sem bagunçar a ordem cronológica!
  - **Preserva flags originais**: mantém status de lido (`\Seen`), respondido (`\Answered`), sinalizado (`\Flagged`), etc.
- **Exportação para Arquivo PST do Microsoft Outlook (.pst)**:
  - Cria arquivos `.pst` nativos do Outlook com todas as pastas e mensagens.
  - Converte corpos HTML/texto puro e anexa arquivos binários perfeitamente.
  - Desconecta a PST da sessão do Outlook com segurança para transporte ou abertura em qualquer computador.
- **Exportação para Diretório de Arquivos .EML**:
  - Salva cópias dos e-mails individuais no formato padrão RFC 822 estruturados em pastas.
- **Exportação para Arquivo .MBOX**:
  - Gera arquivo padrão RFC 4155 compatível com Mozilla Thunderbird e Apple Mail.

### 3. Pesquisa e Visualizador Integrado (Estilo MailStore)
- **Motor de Busca em Texto Completo (FTS5)**: Pesquisa em milissegundos em milhões de e-mails por assunto, remetente, destinatário e corpo.
- **Filtros Avançados**: Filtre por remetente, intervalo de datas e mensagens com anexos (📎).
- **Visualizador Seguro de E-mails**:
  - Exibe cabeçalhos completos (De, Para, Cc, Assunto, Data).
  - Alternância entre visualização HTML formatada (em sandbox seguro) e Texto Puro.
  - Lista de anexos com tamanho e download com 1 clique.
  - Botão de download do arquivo `.eml` original.

### 4. Navegação no Arquivo (Folder Tree)
- Navegação clássica idêntica ao MailStore Home: árvore de pastas à esquerda com badges numéricos de quantidade de mensagens por pasta.
- Clique em qualquer pasta para abrir a listagem instantânea de mensagens.

### 5. Auditoria de Integridade e Histórico
- **Verificador de Integridade SHA-256**: Valida a integridade física de todos os arquivos `.eml` no disco contra o banco de dados e detecta dados corrompidos ou ausentes.
- **Registro Detalhado de Operações**: Histórico de sincronizações, migrações e erros com timestamps.

---

## Estrutura do Projeto

```
mailbackup/
├── mailstore/
│   ├── config.py         # Configurações de diretórios e provedores conhecidos
│   ├── models.py         # Modelos de dados Pydantic
│   ├── database.py       # Banco SQLite com FTS5 e controle de transações
│   ├── parser.py         # Extrator RFC 822 / MIME, anexos e SHA-256
│   ├── imap_engine.py    # Cliente IMAP incremental resiliente
│   ├── pst_engine.py     # Gerador nativo de PST via automação MAPI do Outlook
│   ├── export_engine.py  # Exportação para IMAP (APPEND), EML e MBOX
│   ├── verifier.py       # Auditor de integridade criptográfica SHA-256
│   ├── server.py         # Servidor REST API e servidor web embutido (FastAPI)
│   └── web/              # Interface Web Desktop (HTML5, CSS3, JS Puro)
│       ├── index.html
│       ├── css/app.css
│       └── js/app.js
├── cli.py                # Interface de linha de comando completa
├── main.py               # Ponto de entrada que inicia o app e abre o navegador
├── iniciar.bat           # Executável com 2 cliques para Windows
├── backup_agendado.bat   # Script pronto para o Agendador de Tarefas do Windows
├── requirements.txt      # Dependências do projeto
└── tests/                # Testes automatizados (16 testes cobrindo todo o fluxo)
```

---

## Como Executar

### 1. Pré-requisitos
- Python 3.10 ou superior (testado no Python 3.12).
- Para exportação PST nativa: Windows com Microsoft Outlook instalado. Para IMAP, EML e MBOX: qualquer ambiente.

### 2. Instalação das dependências
```bash
pip install -r requirements.txt
```

### 3. Iniciar a Aplicação Gráfica
Basta dar um duplo clique no arquivo `iniciar.bat` ou executar:
```bash
python main.py
```
O servidor iniciará em `http://127.0.0.1:8000` e a interface gráfica abrirá automaticamente no seu navegador padrão.

---

## Uso da Linha de Comando (CLI)

O MailStore Backup possui suporte completo a linha de comando para automação silenciosa:

### Listar e Gerenciar Perfis
```bash
# Listar perfis cadastrados
python cli.py profiles

# Criar um novo perfil IMAP
python cli.py add-profile --name "Gmail Pessoal" --host "imap.gmail.com" --port 993 --ssl "SSL" --user "meuemail@gmail.com" --password "minhasenhadeapp"
```

### Executar Arquivamento
```bash
# Arquivar por ID ou Nome do perfil
python cli.py backup --profile "Gmail Pessoal"
```

### Exportar / Migrar para Outro Servidor IMAP
```bash
python cli.py export-imap --profile "Gmail Pessoal" --target-host "imap.novoprovedor.com" --target-user "novo@novoprovedor.com" --target-pass "senha123" --prefix "Restaurado"
```

### Exportar para Arquivo PST do Outlook
```bash
python cli.py export-pst --profile "Gmail Pessoal" --output "C:\Users\Usuario\Desktop\backup.pst"
```

### Exportar para Pasta de Arquivos .EML
```bash
python cli.py export-eml --profile "Gmail Pessoal" --output-dir "C:\Users\Usuario\Documents\Emails_EML"
```

### Exportar para Arquivo .MBOX
```bash
python cli.py export-mbox --profile "Gmail Pessoal" --output "C:\Users\Usuario\Documents\arquivo.mbox"
```

### Pesquisar no Arquivo
```bash
python cli.py search "fatura internet"
```

### Auditoria de Integridade
```bash
python cli.py verify
```

---

## Automação no Windows (Task Scheduler)

Para automatizar backups diários ou semanais:
1. Abra o **Agendador de Tarefas do Windows** (`taskschd.msc`).
2. Clique em **Criar Tarefa Básica**.
3. Escolha a frequência (ex: diariamente às 22:00).
4. Na ação, selecione **Iniciar um programa**:
   - Programa/script: `C:\Github\mailbackup\backup_agendado.bat`
   - Adicionar argumentos: `"NomeDoSeuPerfil"`
   - Iniciar em: `C:\Github\mailbackup`

---

## Execução dos Testes Automatizados

O projeto inclui uma suíte completa de testes com mock de servidor IMAP e testes de ponta a ponta:

```bash
python -m unittest discover tests
```

---

## Licença

Desenvolvido para arquivamento e proteção contínua de dados de e-mail.
