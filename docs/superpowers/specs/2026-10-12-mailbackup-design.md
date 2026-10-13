# mailbackup — Design

Data: 2026-10-12

## 1. Objetivo

Ferramenta em Rust, no estilo do MailStore Home, que arquiva contas de e-mail via IMAP em um arquivo local e restaura/exporta esse arquivo para qualquer servidor IMAP (ou pasta de `.eml`). Uso pessoal/pequena escala (algumas contas), Windows.

Fluxo principal: **origem IMAP → arquivo local → exportação/restauração**. A migração direta (estilo guia HostGator) é um atalho `migrate` = `archive` + `export`.

Referência de funcionalidades: manual do MailStore Home (help.mailstore.com/en/home).

### Critérios de sucesso (v1)
- Arquivar uma conta IMAP completa de forma incremental e retomável.
- Exportar o arquivo para outro servidor IMAP preservando pastas, flags e data original, sem duplicar em reexecuções.
- Exportar para pasta de `.eml`.
- `verify` detecta arquivos ausentes ou corrompidos.
- Nenhuma senha em disco fora do cofre do Windows.

## 2. Escopo por fatias

| Versão | Conteúdo |
|---|---|
| v1 | Arquivar via IMAP (incremental, retomável), exportar para IMAP e `.eml`, `verify`, `reindex`, perfis, CLI, `migrate` |
| v2 | Busca FTS5 (operadores do MailStore: `from:`, `subject:`, `*`, `?`, `"frase"`, `-`, `messageid:`), POP3, GUI (egui) |
| v3 | Criptografia do arquivo com senha e chave de recuperação, OAuth2 (Gmail/Microsoft 365), agendamento |

**Fora de escopo (sem previsão):** importar de Outlook/Exchange/Thunderbird, ler PST, exportar `.msg`/mbox/SMTP, indexação de anexos via iFilter.

## 3. Arquitetura

Workspace Cargo, abordagem **síncrona com threads** (uma thread por conta, pool pequeno de conexões IMAP). Lógica atrás de traits para trocar a implementação (inclusive para async) sem reescrever o motor.

```
mailbackup/
├─ crates/
│  ├─ core/       biblioteca com toda a lógica
│  │   profile     perfis (origem/destino, pastas, datas) em TOML
│  │   autoconfig  detecção de servidor por domínio
│  │   auth        trait Authenticator (v1: senha / senha de app)
│  │   source      trait MailSource -> ImapSource
│  │   target      trait MailTarget -> ImapTarget, EmlDirTarget
│  │   store       trait Store      -> FsSqliteStore
│  │   sync        motor: arquivar, exportar, retomar
│  │   verify      verificação de integridade
│  │   secrets     senhas no Windows Credential Manager (keyring)
│  └─ cli/        clap, comandos finos sobre o core
└─ (futuro) gui/  egui, usa apenas a API do core
```

Regras:
- `core` não depende de CLI/GUI; fala com o exterior só por traits, permitindo fakes em memória nos testes.
- O motor emite eventos de progresso por canal `mpsc`; CLI e GUI apenas os exibem.
- Senhas nunca ficam no TOML; apenas no cofre do Windows.

Crates previstos: `imap`/`rustls-imap`, `rusqlite`, `rustls`, `mail-parser`, `clap`, `serde`/`toml`, `keyring`, `sha2`, `proptest` (testes).

### Comandos CLI (v1)
`profile add|list|edit|rm`, `archive <perfil>`, `export <perfil> --to imap|eml`, `migrate <perfil>`, `verify`, `reindex`, `status`.

## 4. Formato do arquivo

```
<arquivo>/
├─ index.db            SQLite (WAL)
├─ archive.toml        versão do formato, id do arquivo
└─ mail/<conta>/<pasta>/<aa>/<sha256>.eml
```

- `.eml` é a mensagem exata recebida do servidor, nomeada pelo SHA-256 do conteúdo; `<aa>` = 2 primeiros caracteres do hash.
- Deduplicação por hash: a mesma mensagem em várias pastas/contas é um arquivo; as ocorrências ficam em `message_location`.
- Gravação segura: escreve em `.tmp`, `fsync`, renomeia; só então insere no SQLite em transação. Falha no meio nunca deixa o índice apontando para arquivo incompleto.
- O índice é cache de consulta: `reindex` o reconstrói dos `.eml`; UID de origem e flags são repostos no próximo `archive`.
- Versão do formato em `archive.toml` e `PRAGMA user_version`; migrações automáticas.

### Tabelas SQLite
| Tabela | Campos principais |
|---|---|
| `account` | id, nome, servidor, usuário |
| `folder` | id, account_id, caminho, `uidvalidity`, `last_uid` |
| `message` | id, `sha256` (único), tamanho, data, remetente, destinatários, assunto, `message_id`, caminho do `.eml`, data de arquivamento |
| `message_location` | message_id, folder_id, `uid`, flags |
| `sync_run` | id, perfil, início, fim, contagens, erros |
| `export_state` | destino, message_id, status (controle de exportação incremental) |
| `message_fts` | (v2) tabela virtual FTS5 |

## 5. Fluxo de sincronização

### Arquivar
1. Conectar com TLS obrigatório (993 ou STARTTLS) e validação de certificado; login via `Authenticator`; `LIST` filtrado pelo perfil.
2. Por pasta: `SELECT`, ler `UIDVALIDITY`/`UIDNEXT` e comparar com `folder`.
   - `UIDVALIDITY` igual: buscar UIDs > `last_uid`.
   - Diferente: reconciliar por `Message-ID` + hash (cabeçalhos via `UID FETCH`) e baixar só o que falta.
   - Pasta nova: baixar tudo, respeitando filtro de datas.
3. Baixar em lotes (~50) com `UID FETCH ... BODY.PEEK[]` (não marca como lida na origem).
4. Por mensagem: SHA-256, gravar `.eml` (método seguro), inserir em `message` ou só em `message_location` se o hash existir; `last_uid` avança na mesma transação do lote.
5. Atualizar flags das mensagens já arquivadas.
6. Registrar `sync_run`.

**Retomada:** `last_uid` só avança após o lote gravado; queda de rede ou Ctrl+C retoma do último lote concluído.

### Exportar
- **IMAP:** recria a estrutura de pastas, `APPEND` preservando flags e `INTERNALDATE`; `export_state` torna a operação incremental e sem duplicação.
- **EML:** copia para pasta com a estrutura de pastas da conta.

### Migração direta
`migrate` = `archive` seguido de `export` para a mesma conta.

### Erros
- Falha em mensagem isolada (corrompida, grande demais, recusada): registrada no `sync_run`, segue adiante, listada no final.
- Rede/limite de taxa: reconexão com recuo exponencial e poucas tentativas por lote; se esgotar, termina com código de saída "parcial" e retoma depois.
- Autenticação: falha imediata, sem repetir tentativas (evita bloqueio da conta).

## 6. Testes
- **Unitários do core** contra fakes em memória de `MailSource`/`MailTarget`/`Store` (sync, retomada, dedup, reconciliação de `UIDVALIDITY`).
- **Propriedades (`proptest`):** interromper em qualquer ponto (inclusive entre o `.eml` e a transação) e retomar produz o mesmo resultado final.
- **`FsSqliteStore` real** em diretórios temporários: `reindex`, `verify` detectando `.eml` removido/corrompido.
- **Integração:** container Dovecot com mensagens de exemplo, `archive` e `export` ponta a ponta (`#[ignore]`, roda no CI ou sob demanda).
- **Segurança:** sem senha em logs/TOML; TLS e certificado validados.

## 7. Pré-requisitos
Toolchain Rust stable (rustup) instalada; Docker opcional para os testes de integração.
