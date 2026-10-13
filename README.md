# mailbackup

Ferramenta de linha de comando (Rust) que **arquiva caixas de e-mail IMAP** em arquivos `.eml`
locais, com um índice SQLite, e **migra** esse arquivo para outra conta IMAP ou para uma pasta de
`.eml`. Foi pensada para Windows, mas o código é portátil.

- Arquivamento incremental e retomável (pode ser interrompido a qualquer momento).
- Mensagens guardadas byte a byte, deduplicadas por SHA-256.
- Exportação para outra conta IMAP (com pastas, flags e data interna) ou para uma pasta de `.eml`.
- Senhas **nunca** vão para o disco em texto: ficam no cofre de credenciais do sistema.

> Versão 1: apenas IMAP, apenas senha (veja [Limitações conhecidas](#limitações-conhecidas)).

## Instalação

Requisitos: [Rust](https://rustup.rs) (edição 2021, versão recente) e, no Windows, as
**Visual Studio Build Tools** com a carga de trabalho "Desenvolvimento para desktop com C++" (o
linker MSVC).

```
cargo build --release
```

O executável fica em `target\release\mailbackup.exe`. Copie-o para uma pasta no `PATH` se quiser.
`mailbackup --help` e `mailbackup <comando> --help` mostram todas as opções.

## Início rápido

### 1. Criar um perfil

Um perfil descreve a conta de origem, onde fica o arquivo e (opcionalmente) o destino da migração.

Com detecção automática do servidor a partir do e-mail (`imap.<domínio>:993`, TLS implícito; há
exceções conhecidas para Gmail, Outlook/Hotmail/Live, Yahoo e iCloud):

```
mailbackup profile add pessoal --email eu@exemplo.com --archive-dir D:\Backups\pessoal
```

Ou manualmente:

```
mailbackup profile add trabalho --host imap.empresa.com --port 993 --tls implicit ^
    --user eu@empresa.com --archive-dir D:\Backups\trabalho
```

Opções úteis (também aceitas por `profile edit`): `--folder <PASTA>` (repetível; padrão: todas as
pastas), `--since AAAA-MM-DD` (só e-mail datado a partir do dia), `--tls implicit|starttls`.

A senha é pedida no terminal (sem eco). Em scripts, passe-a por **uma linha da entrada padrão**
com `--password-stdin` (nunca como argumento de linha de comando):

```
echo senha | mailbackup profile add pessoal --email eu@exemplo.com --archive-dir D:\Backups\pessoal --password-stdin
```

Destino da migração (opcional, pode ser definido depois com `profile edit`):

- outra conta IMAP: `--target-imap-host`, `--target-imap-user`, `--target-imap-port`,
  `--target-imap-tls` — com `--password-stdin`, a senha do destino é a **segunda** linha da entrada;
- uma pasta de `.eml`: `--target-eml D:\Backups\eml-saida`.

Outros comandos de perfil: `profile list`, `profile show <nome>` (nunca mostra senhas),
`profile edit <nome> ...` (com `--clear-folders`, `--clear-since`, `--clear-target`),
`profile set-password <nome> [--target] [--password-stdin]` e `profile rm <nome>` (apaga o perfil e
as senhas guardadas; **a pasta do arquivo é mantida**).

### 2. Arquivar

```
mailbackup archive pessoal
```

Baixa só o que é novo desde a última execução. Pode ser repetido à vontade; interrompê-lo
(Ctrl+C, queda de energia) não corrompe o arquivo, e a próxima execução continua de onde parou.

### 3. Ver e conferir

```
mailbackup status pessoal     # contagem de mensagens por pasta
mailbackup verify pessoal     # confere se cada .eml existe e bate com o hash do índice
```

### 4. Exportar / migrar

```
mailbackup export pessoal --to imap    # para a conta IMAP do perfil
mailbackup export pessoal --to eml     # para a pasta de .eml do perfil
mailbackup migrate pessoal             # archive + export para o destino do perfil
```

A exportação é incremental: o que já foi enviado a um destino não é reenviado. O `--to` deve
combinar com o tipo de destino configurado no perfil.

### 5. Reconstruir o índice

```
mailbackup reindex pessoal
```

Recria `index.db` a partir dos `.eml` (útil se o índice for apagado ou corrompido). Se só o
`index.db` sumiu, `verify`, `status` e `export` recusam rodar (código 1) e mandam usar `reindex`.
O `reindex` é *best-effort*: os nomes de pasta vêm do caminho em disco (veja abaixo) e as UIDs não
são recuperadas; a próxima execução de `archive` reconstrói as pastas exatas (baixa as mensagens de novo,
mas a deduplicação por hash evita duplicatas).

## Onde ficam as configurações e as senhas

- **Perfis:** `%APPDATA%\mailbackup\profiles\<nome>.toml` no Windows (diretório de configuração do
  usuário nos outros sistemas). Mude com `--config-dir <PASTA>` ou a variável
  `MAILBACKUP_CONFIG_DIR`. O arquivo TOML **nunca** contém senhas.
- **Senhas:** no cofre do sistema (Windows Credential Manager; Keychain no macOS; Secret Service no
  Linux), sob as chaves `mailbackup:<perfil>:source` e `mailbackup:<perfil>:target`.

## Formato do arquivo em disco

```
<archive-dir>/
  archive.toml                 identificador e versão do formato do arquivo
  index.db                     índice SQLite (cache; pode ser reconstruído com `reindex`)
  mail/<conta>/<pasta>/<aa>/<sha256>.eml
```

- `<conta>` é o nome do perfil; `<pasta>` é o nome da pasta IMAP com caracteres inválidos no
  Windows trocados por `_` (inclusive `/` entre níveis, p. ex. `Trabalho/Projetos` vira
  `Trabalho_Projetos`) e abreviado com um sufixo de hash quando muito longo; `<aa>` são os dois
  primeiros caracteres do SHA-256.
- Cada `.eml` é a mensagem **exatamente** como veio do servidor; o nome é o SHA-256 dos bytes.
  Flags, datas e a origem (pasta/UID) ficam só no `index.db`.

### Como fazer backup do próprio arquivo

Copie a pasta inteira (`archive-dir`) **com o mailbackup fechado** — durante uma execução o
SQLite pode estar no meio de uma gravação. Os `.eml` são a fonte da verdade; o `index.db` é só um
cache que o `reindex` reconstrói. (Se copiar sem o `index.db`, o `archive.toml` precisa ir junto.)

## Códigos de saída

| Código | Significado |
|---|---|
| `0` | Sucesso |
| `1` | Erro (configuração, rede, credenciais, uso incorreto da linha de comando) ou `verify` achou arquivos ausentes/corrompidos |
| `2` | Sucesso parcial: o comando terminou, mas uma ou mais mensagens falharam (listadas na saída de erro) |

`--help` e `--version` saem com `0`.

## Testes

```
cargo test --workspace --features testing
cargo test -p mailbackup --features test-secrets
```

O primeiro roda os testes do núcleo (com servidores IMAP simulados) e unitários; o segundo, os
testes de ponta a ponta da CLI com um cofre de senhas **em arquivo texto simples**, apenas para
testes (a feature `test-secrets` nunca deve ser usada em um build de uso real).

### Teste com Dovecot real (opcional, ignorado por padrão)

`crates/cli/tests/e2e_dovecot.rs` arquiva uma conta `origem`, exporta para `destino` no mesmo
Dovecot e compara contagens, flags e SHA-256 por pasta. Precisa de Docker e de um certificado em
que o sistema confie (o mailbackup **sempre** valida o TLS). Resumo — detalhes em
[`tests/dovecot/README.md`](tests/dovecot/README.md):

```
sh tests/dovecot/make-certs.sh
certutil -user -addstore Root tests\dovecot\certs\ca.crt       (Windows)
docker compose -f tests/dovecot/docker-compose.yml up -d
cargo test -p mailbackup --features test-secrets --test e2e_dovecot -- --ignored --nocapture
docker compose -f tests/dovecot/docker-compose.yml down
```

**Atenção:** a máquina de desenvolvimento não tinha Docker. Esse teste e o `docker-compose.yml`
foram apenas compilados/verificados com clippy e **nunca foram executados**.

## Limitações conhecidas

Fora do escopo da v1 (previstos para v2/v3): POP3, busca, interface gráfica, criptografia do
arquivo, OAuth2 e agendamento automático.

- **Gmail e Microsoft 365 / Outlook:** exigem *senha de aplicativo* (ou IMAP básico habilitado),
  pois ainda não há OAuth2.
- **Sem timeout de conexão/leitura:** se o servidor travar sem fechar a conexão, o comando fica
  esperando.
- **Possível duplicata no destino:** se a conexão cair durante um `APPEND` na exportação IMAP, a
  mensagem é tratada como falha e pode ser enviada de novo na próxima execução, duplicando-a se o
  servidor já a tinha gravado. O mesmo vale se o processo morrer entre o envio e o registro.
- **`/` dentro do nome de uma pasta** em servidores cujo delimitador é outro (por exemplo `.`) é
  ambíguo: o mailbackup usa `/` como separador de níveis.
- **Flags após reset de UIDVALIDITY:** as mensagens baixadas antes do reset deixam de ter as flags
  atualizadas nas execuções seguintes (nunca ficam erradas, apenas deixam de ser sincronizadas;
  nenhuma mensagem se perde).
- **`--since` tem granularidade de dia** (a busca IMAP `SINCE` ignora a hora e o fuso).
- **Uso de memória:** cada lote de até 50 mensagens é mantido na memória; caixas com muitas
  mensagens muito grandes podem consumir bastante RAM.
- **`reindex` é aproximado** (veja acima) e confia no conteúdo dos `.eml` que encontra.
- **Caminhos longos no Windows:** nomes de pasta muito longos/profundos podem esbarrar no limite de
  caminho; nomes que diferem só por maiúsculas/minúsculas podem colidir no Windows.
- **Não verificado em CI:** os caminhos reais do cofre de credenciais (Windows Credential Manager),
  de TLS/STARTTLS e do Dovecot **não foram exercitados** em testes automáticos, porque a máquina de
  desenvolvimento não tinha Docker; os testes usam servidores IMAP simulados e um cofre em arquivo.
  Teste com uma conta descartável antes de confiar no arquivo.
