# Dovecot de teste (SOMENTE TESTE)

Servidor Dovecot descartável usado pelos testes `#[ignore]` de integração:

- `crates/cli/tests/e2e_dovecot.rs` — `e2e_archive_then_export_to_second_account` (arquiva `origem`, exporta para `destino`);
- `crates/core/tests/imap_dovecot.rs` — `archive_from_dovecot` (usa o usuário `testuser`).

> **Estado de verificação:** estes arquivos foram escritos numa máquina **sem Docker**. O teste Rust
> compila e passa no clippy, mas o compose, a configuração do Dovecot e o teste de ponta a ponta
> **nunca foram executados**. Trate-os como não testados até a primeira execução real; ajustes
> (versão da imagem, usuário `vmail`, caminhos) podem ser necessários.

## Conteúdo

| Arquivo | Para quê |
|---|---|
| `docker-compose.yml` | Dovecot 2.3 (`dovecot/dovecot:2.3.21.1`) com IMAPS na porta 993 |
| `conf/dovecot.conf` | Configuração completa (substitui a da imagem): passwd-file, Maildir `LAYOUT=fs` (separador `/`), TLS obrigatório |
| `users` | Usuários `origem`, `destino`, `testuser` |
| `make-certs.sh` | Gera uma CA local e um certificado de servidor para `localhost` (`certs/`, ignorado pelo git) |
| `seed/` | Mensagens de exemplo (ver `seed/folders/README.md`) |

Credenciais (**somente teste, nunca reutilize**):

| Usuário | Senha |
|---|---|
| `origem` | `origem-test-only` |
| `destino` | `destino-test-only` |
| `testuser` | `testpass` |

## Passo a passo

1. Gere os certificados (precisa do `openssl`; o Git for Windows traz um):
   `sh tests/dovecot/make-certs.sh`
2. Faça o sistema operacional **confiar na CA** `tests/dovecot/certs/ca.crt`. O mailbackup sempre
   valida o certificado (não existe opção para desligar isso) e usa as raízes do sistema.
   - Windows (PowerShell, usuário atual; o Windows mostra uma janela de confirmação):
     `certutil -user -addstore Root tests\dovecot\certs\ca.crt`
     Para remover depois: `certutil -user -delstore Root "mailbackup test CA"`.
   - Linux: copie para `/usr/local/share/ca-certificates/mailbackup-test-ca.crt` e rode
     `sudo update-ca-certificates` (ou exporte `SSL_CERT_FILE=.../ca.crt`).
   - macOS: `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ca.crt`.
   A CA vale 30 dias; remova-a do repositório de confiança quando terminar.
3. Suba o servidor: `docker compose -f tests/dovecot/docker-compose.yml up -d`
4. Rode o teste de ponta a ponta:
   `cargo test -p mailbackup --features test-secrets --test e2e_dovecot -- --ignored --nocapture`
   O teste popula `origem` sozinho (via IMAP APPEND), então precisa de um servidor **novo**:
   entre execuções use `docker compose -f tests/dovecot/docker-compose.yml down` e `up -d`
   (o estado não é persistido de propósito).
5. Para o teste do núcleo (`archive_from_dovecot`), `testuser` precisa ter em `INBOX` exatamente as
   mensagens `seed/*.eml` (nível superior). Entregue-as, por exemplo, com
   `docker compose -f tests/dovecot/docker-compose.yml exec -T dovecot doveadm save -u testuser -m INBOX < tests/dovecot/seed/001.eml`
   (comando não testado) e rode
   `MAILBACKUP_TEST_IMAP_USER=testuser cargo test -p mailbackup-core --features testing -- --ignored archive_from_dovecot`.

## Variáveis de ambiente

e2e: `MAILBACKUP_E2E_HOST` (localhost), `MAILBACKUP_E2E_PORT` (993),
`MAILBACKUP_E2E_ORIGEM_USER` / `_PASSWORD`, `MAILBACKUP_E2E_DESTINO_USER` / `_PASSWORD`.
Teste do núcleo: `MAILBACKUP_TEST_IMAP_HOST|PORT|USER|PASSWORD`.

## O que o e2e confere

Semeia `origem` (6 mensagens: quatro no INBOX, incluindo uma **sem Message-ID** e uma com
**anexo de ~5 MB** gerado pelo próprio teste, uma na pasta `Álbum de Fotos` — acento e espaço — e uma
na pasta aninhada `Trabalho/Projetos 2024`), roda `profile add`, `archive`, `verify` e
`export --to imap` com o binário real, compara por pasta contagem, flags e SHA-256 dos bytes entre
`origem` e `destino`, e confirma que um segundo `export` não exporta nada (`added: 0`).
