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

- `--folder`: pastas aninhadas usam **sempre `/`** entre os níveis (`--folder "Trabalho/Projetos"`),
  mesmo que o servidor use outro delimitador (por exemplo `.`); `INBOX` não diferencia maiúsculas
  de minúsculas. Uma pasta pedida que não existe no servidor **não** é ignorada em silêncio: aparece
  como falha (`failed: folder <nome>: folder not found`) e o comando sai com código `2`.
- Mudar o `--since` (ou removê-lo com `--clear-since`) num perfil que já tem arquivo faz o próximo
  `archive` percorrer as pastas desde o início e baixar só o que ainda falta (por exemplo, o e-mail
  mais antigo que o filtro anterior excluía).
- `--archive-dir` e `--target-eml` são guardados como caminho **absoluto** (um caminho relativo é
  resolvido a partir da pasta atual no momento do `profile add`/`edit`).
- Nomes de perfil não diferenciam maiúsculas de minúsculas: com `work` existente, `profile add Work`
  é recusado.

A senha é pedida no terminal (sem eco) — é o modo recomendado. Em scripts, use
`--password-stdin`, que lê a senha da **primeira linha da entrada padrão** (nunca a passe como
argumento de linha de comando). O jeito mais seguro é redirecionar um arquivo que contenha só a
senha e apagá-lo em seguida:

```
mailbackup profile add pessoal --email eu@exemplo.com --archive-dir D:\Backups\pessoal --password-stdin < senha.txt
```

Evite `echo senha | mailbackup ...`: no `cmd.exe` o espaço antes do `|` passa a fazer parte da
senha (`"senha "`), e a senha fica no histórico do shell. No PowerShell 5.1 o pipe pode estragar
caracteres não ASCII; prefira o prompt interativo ou o redirecionamento de arquivo.

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
Uma pasta que o servidor não deixa abrir (permissão, apagada no meio da execução, erro do servidor)
é listada como falha e as demais pastas continuam sendo arquivadas (código de saída `2`). Uma
mensagem que falha é tentada de novo nas próximas execuções, sem baixar outra vez as que já estão
no arquivo.

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
combinar com o tipo de destino configurado no perfil. Um `.eml` local ausente ou que não bate com o
seu SHA-256 **nunca é enviado**: a mensagem é listada como falha (rode `verify`) e o resto continua.

### 5. Reconstruir o índice

```
mailbackup reindex pessoal
```

Recria `index.db` a partir dos `.eml` (útil se o índice for apagado ou corrompido). Se só o
`index.db` sumiu, `verify`, `status` e `export` recusam rodar (código 1) e mandam usar `reindex`.

O que o `reindex` recupera: uma linha por mensagem (hash, tamanho, cabeçalhos) e a pasta onde o
`.eml` está gravado, com o **nome exato** da conta e da pasta lido dos arquivos `.account`/`.folder`
(veja abaixo; arquivos criados antes deles caem no nome da pasta em disco, que pode estar alterado).
O que ele **não** recupera: UIDs, flags, data interna, as outras pastas em que a mesma mensagem
aparecia (o `.eml` fica só na primeira) e o registro do que já foi exportado. Depois de um
`reindex`:

- rode `archive`: ele baixa as pastas de novo (a deduplicação por hash evita cópias no disco) e
  restaura UIDs, flags e as localizações em todas as pastas;
- um `export --to imap` **reenviaria tudo**, porque o registro de exportação se perdeu: o destino
  IMAP ficaria com mensagens duplicadas. Só exporte para um destino IMAP vazio, ou não exporte de
  novo para o mesmo destino. A exportação para `.eml` é idempotente (não duplica arquivos).

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
  mail/<conta>/.account        nome exato da conta (UTF-8)
  mail/<conta>/<pasta>/.folder nome exato da pasta IMAP (UTF-8, níveis separados por "/")
  mail/<conta>/<pasta>/<aa>/<sha256>.eml
```

- `<conta>` é o nome do perfil; `<pasta>` é o nome da pasta IMAP com caracteres inválidos no
  Windows trocados por `_` (inclusive `/` entre níveis, p. ex. `Trabalho/Projetos 2024` vira
  `Trabalho_Projetos 2024-<hash>`; nomes que começam com `.` ganham um `_` na frente e o sufixo
  de hash) e abreviado quando muito longo — por isso o nome verdadeiro fica
  nos arquivos `.account` e `.folder`, que o `reindex` usa; `<aa>` são os dois primeiros caracteres
  do SHA-256.
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
| `2` | Sucesso parcial: o comando terminou, mas uma ou mais mensagens ou pastas falharam (listadas na saída de erro), inclusive uma pasta de `--folder` que não existe no servidor |

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
certutil -user -delstore Root "mailbackup test CA"            (Windows: remova a CA ao terminar)
```

A CA de teste só pode valer para `localhost`/`127.0.0.1` (*name constraints*) e a chave privada dela
é apagada pelo próprio `make-certs.sh`; mesmo assim, **remova-a do repositório de confiança** assim
que terminar o teste.

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
- **`reindex` não recupera UIDs, flags, outras pastas nem o registro de exportação** (veja acima):
  rode `archive` depois dele e não reexporte para um destino IMAP que já recebeu as mensagens.
- **Não rode dois mailbackup ao mesmo tempo no mesmo arquivo** (ainda não há trava): pode dar
  "database is locked" ou conflito entre arquivos temporários. O modo WAL do SQLite não funciona
  em pastas de rede (SMB/NAS): mantenha o arquivo num disco local.
- **Mudar o servidor/e-mail de um perfil** (`profile edit --host/--email`) mantém o estado das
  pastas da conta antiga; para outra conta, crie outro perfil com outro arquivo.
- **Rede instável:** quando as novas tentativas de conexão se esgotam, o comando sai com código `1`
  (não `2`) e o resumo parcial não é impresso (o que já foi gravado continua no arquivo).
- **A feature `test-secrets`** guarda senhas em arquivo texto simples; nunca use um binário
  compilado com ela (o `cargo test` deixa um em `target/`).
- **Caminhos longos no Windows:** nomes de pasta muito longos/profundos podem esbarrar no limite de
  caminho; nomes que diferem só por maiúsculas/minúsculas podem colidir no Windows.
- **Não verificado em CI:** os caminhos reais do cofre de credenciais (Windows Credential Manager),
  de TLS/STARTTLS e do Dovecot **não foram exercitados** em testes automáticos, porque a máquina de
  desenvolvimento não tinha Docker; os testes usam servidores IMAP simulados e um cofre em arquivo.
  Teste com uma conta descartável antes de confiar no arquivo.
