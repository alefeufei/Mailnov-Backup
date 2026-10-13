# mailbackup v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Arquivar contas IMAP de forma incremental e retomável em um arquivo local (`.eml` + SQLite) e exportá-lo para outro servidor IMAP ou pasta de `.eml`, via CLI.

**Architecture:** Workspace Cargo com `crates/core` (biblioteca síncrona, lógica atrás dos traits `MailSource`, `MailTarget`, `Store`) e `crates/cli` (clap, fino). O motor é testado contra fakes em memória; IMAP e SQLite reais ficam em implementações separadas.

**Tech Stack:** Rust stable, `imap`, `rusqlite` (bundled), `mail-parser`, `sha2`, `clap`, `serde`/`toml`, `keyring`, `thiserror`, `indicatif`, `proptest`, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-10-12-mailbackup-design.md`

## Global Constraints

- Sem async: threads + `std::sync::mpsc` para eventos de progresso.
- TLS obrigatório (993 ou STARTTLS) com validação de certificado; sem opção de desligar a validação.
- Senhas nunca em TOML, logs ou mensagens de erro; só em `keyring` (Windows Credential Manager).
- `.eml` guardado byte a byte como veio do servidor, nomeado `mail/<conta>/<pasta>/<aa>/<sha256>.eml`, `<aa>` = 2 primeiros caracteres do hash.
- Gravação: `.tmp` → `fsync` → rename → só então a transação SQLite; `last_uid` avança na mesma transação do lote.
- Download com `BODY.PEEK[]` (não marca como lida); lotes de 50 mensagens.
- Falha em uma mensagem não aborta a execução; falha de autenticação aborta sem retentar; rede: recuo exponencial, poucas tentativas.
- `archive.toml` e `PRAGMA user_version` guardam a versão do formato (v1 = 1).
- `core` não depende de `cli`. Backup nunca apaga nada na origem nem no arquivo.

## Review Focus

1. Nome de pasta com caracteres inválidos no Windows (`:`, `?`, `*`, `/`), acentos ou UTF-7 modificado do IMAP → deve virar caminho válido e estável. (Task 2)
2. Mensagem sem `Message-ID`, sem `Date` ou com corpo vazio → arquivada normalmente, nunca descartada. (Task 3)
3. Mesma mensagem em duas pastas → um só `.eml`, duas `message_location`. (Task 2)
4. Queda entre gravar o `.eml` e commitar a transação → retomada reaproveita o arquivo órfão e não duplica nem falha. (Task 3)
5. Mensagem apagada na origem depois de arquivada → continua no arquivo e em `verify` sem erro. (Task 3)

---

### Task 1: Workspace, tipos e erros

**Files:**
- Create: `Cargo.toml`, `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/types.rs`, `crates/core/src/error.rs`, `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`, `.gitignore`
- Test: `crates/core/src/types.rs` (módulo `tests`)

**Interfaces:**
- Produces (`types.rs`):
  - `struct RawMessage { uid: u32, flags: Vec<String>, internal_date: Option<i64>, bytes: Vec<u8> }`
  - `struct FolderState { uidvalidity: u32, uidnext: u32 }`
  - `enum Event { FolderStarted{folder:String,total:usize}, MessageArchived{folder:String,uid:u32}, MessageFailed{folder:String,uid:u32,reason:String}, Finished(RunSummary) }`
  - `struct RunSummary { added: usize, duplicates: usize, failed: usize }`
  - `fn sha256_hex(bytes: &[u8]) -> String`
- Produces (`error.rs`): `enum Error { Io(std::io::Error), Db(String), Imap(String), Auth(String), Config(String), Corrupt(String) }` com `thiserror`; `type Result<T> = std::result::Result<T, Error>`.

- [ ] **Step 1: Write the failing test** `sha256_hex_matches_known_vector`: `sha256_hex(b"abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"`.
- [ ] **Step 2: Run** `cargo test -p mailbackup-core` — Expected: FAIL (crate/função inexistente).
- [ ] **Step 3: Criar o workspace** (`resolver = "2"`, membros `crates/core` e `crates/cli`, crates `mailbackup-core` e `mailbackup`), os tipos e o erro acima; `.gitignore` com `/target`. `git init` na raiz.
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `git add -A && git commit -m "chore: workspace, core types and errors"`

### Task 2: Store (FsSqliteStore)

**Files:**
- Create: `crates/core/src/store/mod.rs` (trait), `crates/core/src/store/fs_sqlite.rs`, `crates/core/src/store/paths.rs`, `crates/core/src/store/schema.sql`
- Test: `crates/core/src/store/fs_sqlite.rs` e `paths.rs` (módulos `tests`, com `tempfile`)

**Interfaces:**
- Consumes: `RawMessage`, `sha256_hex`, `Error`.
- Produces (`store/mod.rs`):
  ```rust
  pub struct StoredFolder { pub uidvalidity: u32, pub last_uid: u32 }
  pub struct StoredMessage { pub sha256: String, pub account: String, pub folder: String, pub uid: Option<u32>, pub flags: Vec<String>, pub internal_date: Option<i64> }
  pub struct BatchOutcome { pub added: usize, pub duplicates: usize }
  pub struct VerifyReport { pub checked: usize, pub missing: Vec<String>, pub corrupt: Vec<String> }
  pub trait Store {
      fn folder_state(&self, account: &str, folder: &str) -> Result<Option<StoredFolder>>;
      fn commit_batch(&mut self, account: &str, folder: &str, uidvalidity: u32, batch: &[RawMessage], new_last_uid: u32) -> Result<BatchOutcome>;
      fn has_message_id(&self, account: &str, message_id: &str) -> Result<bool>;
      fn update_flags(&mut self, account: &str, folder: &str, uid: u32, flags: &[String]) -> Result<()>;
      fn messages(&self, account: &str) -> Result<Vec<StoredMessage>>;
      fn read_eml(&self, sha256: &str) -> Result<Vec<u8>>;
      fn export_done(&self, target: &str, sha256: &str, folder: &str) -> Result<bool>;
      fn mark_exported(&mut self, target: &str, sha256: &str, folder: &str) -> Result<()>;
      fn verify(&self) -> Result<VerifyReport>;
      fn reindex(&mut self) -> Result<usize>;
  }
  ```
- Produces (`fs_sqlite.rs`): `FsSqliteStore::open(root: &Path) -> Result<FsSqliteStore>` (cria `archive.toml`, `index.db` em WAL, tabelas `account`, `folder`, `message`, `message_location`, `sync_run`, `export_state` conforme a spec; `PRAGMA user_version = 1`).
- Produces (`paths.rs`): `fn safe_component(name: &str) -> String` (determinístico, válido no Windows, sem `..`, limitado a 80 caracteres, sufixo com hash curto quando sanitizado) e `fn eml_relpath(account: &str, folder: &str, sha256: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**
  - `safe_component_replaces_windows_invalid_chars`: `safe_component("INBOX/a:b?c*")` não contém `/ : ? *` e é igual em duas chamadas.
  - `safe_component_distinguishes_colliding_names`: `"a:b"` e `"a?b"` dão resultados diferentes.
  - `commit_batch_writes_eml_and_index`: 2 mensagens → `added == 2`; o arquivo existe em `eml_relpath`; `folder_state` retorna `last_uid == new_last_uid`.
  - `same_bytes_in_two_folders_is_one_file`: mesma mensagem em `INBOX` e `Sent` → um `.eml` em disco, `messages()` com 2 localizações, `duplicates == 1` na segunda.
  - `commit_batch_is_atomic_on_db_failure`: falha injetada após gravar o `.eml` (cenário de aborto) deixa `folder_state` inalterado.
  - `orphan_eml_from_crash_is_reused`: `.eml` já presente em disco sem linha no banco → `commit_batch` conclui sem erro e sem duplicar.
  - `reopen_is_idempotent`: abrir duas vezes o mesmo diretório não perde dados; `user_version == 1`.
  - `verify_reports_missing_and_corrupt`: apagar um `.eml` e alterar outro → `missing.len()==1`, `corrupt.len()==1`.
  - `reindex_rebuilds_message_rows`: apagar `index.db`, `reindex()` devolve o nº de `.eml` e `messages()` os lista (uid `None`).
- [ ] **Step 2: Run** `cargo test -p mailbackup-core store` — Expected: FAIL.
- [ ] **Step 3: Implement** `FsSqliteStore` e `paths.rs`. Gravação: escrever `.tmp` no mesmo diretório, `File::sync_all`, `rename`; o lote inteiro (linhas de `message`, `message_location`, `folder.last_uid`) em uma única transação. Metadados (remetente, assunto, `Message-ID`, data) via `mail-parser`; ausência de qualquer cabeçalho vira `NULL`, nunca erro. `verify` recalcula o SHA-256 de cada `.eml` indexado.
- [ ] **Step 4: Run** `cargo test -p mailbackup-core store` — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(core): FsSqliteStore with dedup, verify and reindex"`

### Task 3: Motor de arquivamento com fakes

**Files:**
- Create: `crates/core/src/source.rs` (trait), `crates/core/src/sync/mod.rs`, `crates/core/src/sync/archive.rs`, `crates/core/src/testing.rs` (fakes, `#[cfg(test)]` + feature `testing`)
- Test: `crates/core/tests/archive_engine.rs`

**Interfaces:**
- Consumes: `Store` (Task 2), `RawMessage`, `FolderState`, `Event`, `RunSummary`.
- Produces (`source.rs`):
  ```rust
  pub trait MailSource {
      fn list_folders(&mut self) -> Result<Vec<String>>;
      fn select(&mut self, folder: &str) -> Result<FolderState>;
      fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>>;
      fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>>;
      fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>>;
      fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>>;
  }
  ```
- Produces (`sync/archive.rs`):
  ```rust
  pub struct ArchiveOptions { pub account: String, pub folders: Option<Vec<String>>, pub since: Option<i64>, pub batch_size: usize }
  pub fn archive(source: &mut dyn MailSource, store: &mut dyn Store, opts: &ArchiveOptions, events: &Sender<Event>) -> Result<RunSummary>;
  ```
- Produces (`testing.rs`): `FakeSource` (pastas, UIDs, `uidvalidity` configuráveis, falha injetável por UID ou após N lotes) e `MemStore` não é necessário: usar `FsSqliteStore` em `tempfile`.

- [ ] **Step 1: Write the failing tests** (cada um com `FakeSource` + `FsSqliteStore` temporário)
  - `first_run_archives_everything`: 120 mensagens em `INBOX` → `added == 120`, `last_uid` correto (3 lotes de 50).
  - `second_run_fetches_only_new_uids`: segunda execução sem novidades → `added == 0` e `fetch` não é chamado.
  - `resume_after_failure_between_batches`: fonte falha no 2º lote → erro/parcial; nova execução termina com o mesmo conteúdo final que uma execução sem falha.
  - `uidvalidity_change_reconciles_by_message_id`: `uidvalidity` muda, UIDs renumerados → só mensagens com `Message-ID` desconhecido são baixadas; as conhecidas não duplicam.
  - `bad_message_does_not_abort`: um UID com `fetch` falhando → `failed == 1`, as demais arquivadas, evento `MessageFailed` emitido.
  - `message_without_message_id_or_date_is_archived`: bytes `b"Subject: x\r\n\r\n"` e bytes vazios após cabeçalho → `added == 2`.
  - `deleted_at_source_stays_archived`: mensagem some da fonte na 2ª execução → continua em `messages()` e `verify().missing` vazio.
  - `crash_between_eml_and_commit_resumes`: `proptest` com ponto de falha aleatório em (lote, passo) → após retomar, `messages()` e arquivos em disco são iguais aos de uma execução sem falha.
  - `flags_are_updated_for_known_messages`: flag `\Seen` mudada na origem aparece em `messages()` após nova execução.
  - `folder_filter_and_since_are_respected`.
- [ ] **Step 2: Run** `cargo test -p mailbackup-core --test archive_engine` — Expected: FAIL.
- [ ] **Step 3: Implement** `archive`: fluxo da spec §5 (SELECT → comparar `uidvalidity`/`last_uid` → `uids_after` → lotes de `batch_size` → `fetch` → `commit_batch` → atualizar flags → resumo). Em `uidvalidity` diferente: `message_ids` + `has_message_id`, baixar só os desconhecidos (sem `Message-ID` sempre baixa). Falha de `fetch` de uma mensagem: refazer o lote mensagem a mensagem, registrar `MessageFailed` e seguir.
- [ ] **Step 4: Run** o mesmo comando — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(core): incremental, resumable archive engine"`

### Task 4: ImapSource e autenticação

**Files:**
- Create: `crates/core/src/auth.rs`, `crates/core/src/imap_source.rs`
- Test: `crates/core/src/imap_source.rs` (módulo `tests` para a lógica pura de parsing/UTF-7) e `crates/core/tests/imap_dovecot.rs` (`#[ignore]`)

**Interfaces:**
- Consumes: `MailSource`.
- Produces:
  - `trait Authenticator { fn authenticate(&self, client: ImapClientAuth<'_>) -> Result<()> }` e `PasswordAuth { user: String, password: String }` (`ImapClientAuth` é o handle de login do crate `imap`).
  - `ImapSource::connect(host: &str, port: u16, tls: TlsMode, auth: &dyn Authenticator) -> Result<ImapSource>`; `enum TlsMode { Implicit, StartTls }` (sem opção "None").
  - `ImapSource` implementa `MailSource` usando `UID SEARCH`, `UID FETCH ... (FLAGS INTERNALDATE BODY.PEEK[])`, `BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]`, com reconexão e recuo exponencial (3 tentativas) em erros de rede; erro de login → `Error::Auth` sem retentar.
  - `fn decode_folder_name(modified_utf7: &str) -> String`.

- [ ] **Step 1: Write the failing tests**
  - `decode_folder_name_handles_modified_utf7`: `decode_folder_name("Caixa de &AOc-ao")` == `"Caixa de ção"` e `decode_folder_name("INBOX")` == `"INBOX"`.
  - `auth_failure_is_not_retried` (com servidor TCP de mentira que devolve `NO`): exatamente 1 tentativa, `Error::Auth`.
  - Integração `archive_from_dovecot` (`#[ignore]`, requer `docker compose up` de `tests/dovecot/`): arquivar a conta de teste e conferir contagem e hash de uma mensagem conhecida.
- [ ] **Step 2: Run** `cargo test -p mailbackup-core imap_source` — Expected: FAIL.
- [ ] **Step 3: Implement.** Backend TLS: o que o crate `imap` oferecer com validação de certificado (`rustls-tls` se disponível; senão `native-tls`/SChannel). Registrar a escolha em `Cargo.toml` com comentário de uma linha. Mapear erros IMAP para `Error::Imap`, sem incluir a senha em nenhuma mensagem.
- [ ] **Step 4: Run** testes unitários — Expected: PASS; integração manual com `cargo test -- --ignored`.
- [ ] **Step 5: Commit** `git commit -am "feat(core): ImapSource with TLS and password auth"`

### Task 5: Exportação (EML e IMAP)

**Files:**
- Create: `crates/core/src/target.rs` (trait), `crates/core/src/eml_target.rs`, `crates/core/src/imap_target.rs`, `crates/core/src/sync/export.rs`
- Test: `crates/core/tests/export_engine.rs`

**Interfaces:**
- Consumes: `Store`, `StoredMessage`, `Event`.
- Produces:
  ```rust
  pub trait MailTarget {
      fn id(&self) -> String;                                   // chave em export_state
      fn ensure_folder(&mut self, folder: &str) -> Result<()>;
      fn put(&mut self, folder: &str, bytes: &[u8], flags: &[String], internal_date: Option<i64>) -> Result<()>;
  }
  pub fn export(store: &mut dyn Store, target: &mut dyn MailTarget, account: &str, events: &Sender<Event>) -> Result<RunSummary>;
  ```
  - `EmlDirTarget::new(dir: &Path)`; `ImapTarget::connect(host, port, TlsMode, &dyn Authenticator)` (`CREATE` da pasta se faltar, `APPEND` com flags e `INTERNALDATE`).

- [ ] **Step 1: Write the failing tests** (alvo fake em memória + `EmlDirTarget` em `tempfile`)
  - `export_creates_folders_and_puts_each_message_once`.
  - `second_export_sends_nothing`: nova execução → `added == 0` (via `export_state`).
  - `export_preserves_flags_and_internal_date` (alvo fake registra os argumentos).
  - `failure_on_one_message_does_not_abort_and_retries_next_run`.
  - `eml_target_writes_account_folder_structure_with_original_bytes`: bytes idênticos aos do arquivo; nomes de pasta passam por `safe_component`.
- [ ] **Step 2: Run** `cargo test -p mailbackup-core --test export_engine` — Expected: FAIL.
- [ ] **Step 3: Implement** `export` (itera `store.messages(account)`, pula o que `export_done`, `ensure_folder`, `put`, `mark_exported`) e os dois alvos. `ImapTarget` usa o mesmo recuo/TLS do `ImapSource`; teste de integração Dovecot (`#[ignore]`) para `APPEND` real.
- [ ] **Step 4: Run** o mesmo comando — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(core): export engine with EML and IMAP targets"`

### Task 6: Perfis, segredos e autoconfig

**Files:**
- Create: `crates/core/src/profile.rs`, `crates/core/src/secrets.rs`, `crates/core/src/autoconfig.rs`
- Test: módulos `tests` em cada arquivo

**Interfaces:**
- Produces:
  ```rust
  pub struct Endpoint { pub host: String, pub port: u16, pub tls: TlsMode, pub user: String }
  pub struct Profile { pub name: String, pub archive_dir: PathBuf, pub source: Endpoint, pub folders: Option<Vec<String>>, pub since: Option<String> /*YYYY-MM-DD*/, pub target: Option<TargetSpec> }
  pub enum TargetSpec { Imap(Endpoint), EmlDir(PathBuf) }
  pub struct ProfileStore { /* diretório de config */ }
  impl ProfileStore { pub fn new(dir: &Path) -> Self; pub fn save(&self, p: &Profile) -> Result<()>; pub fn load(&self, name: &str) -> Result<Profile>; pub fn list(&self) -> Result<Vec<String>>; pub fn remove(&self, name: &str) -> Result<()>; }
  pub trait SecretStore { fn set(&self, key: &str, secret: &str) -> Result<()>; fn get(&self, key: &str) -> Result<String>; }
  pub struct KeyringSecrets; // implementa SecretStore com `keyring`
  pub fn guess_imap(email: &str) -> Option<Endpoint>; // imap.<domínio>:993, TLS implícito
  ```

- [ ] **Step 1: Write the failing tests**
  - `profile_roundtrips_through_toml`.
  - `profile_toml_never_contains_password`: serializado não contém o campo `password` (o tipo nem tem esse campo).
  - `load_unknown_profile_is_config_error`.
  - `guess_imap_uses_domain_and_port_993`: `guess_imap("a@exemplo.com.br")` → `host == "imap.exemplo.com.br"`, `port == 993`.
  - `since_must_be_valid_date` (`"2024-13-40"` → `Error::Config`).
  - `secret_store_trait_works_with_in_memory_fake` (a implementação `keyring` real não entra em teste automático).
- [ ] **Step 2: Run** `cargo test -p mailbackup-core profile secrets autoconfig` — Expected: FAIL.
- [ ] **Step 3: Implement.** Perfis em `<config>/profiles/<nome>.toml`; chave do segredo: `mailbackup:<perfil>:source|target`.
- [ ] **Step 4: Run** o mesmo comando — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(core): profiles, secrets and autoconfig"`

### Task 7: CLI

**Files:**
- Modify: `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`
- Create: `crates/cli/src/commands.rs`, `crates/cli/src/progress.rs`
- Test: `crates/cli/tests/cli.rs` (`assert_cmd`, perfil `EmlDir` e fonte fake via variável de ambiente de teste não é permitida — usar só comandos que não precisam de rede: `profile`, `status`, `verify`, `reindex`)

**Interfaces:**
- Consumes: tudo do `core`.
- Produces: binário `mailbackup` com `profile add|list|edit|rm`, `archive <perfil>`, `export <perfil> --to imap|eml`, `migrate <perfil>`, `verify <perfil>`, `reindex <perfil>`, `status <perfil>`; código de saída `0` ok, `1` erro, `2` parcial (houve falhas de mensagem ou rede esgotada); `progress.rs` consome o `Receiver<Event>` e mostra barra com `indicatif`.
  - `profile add` pede a senha sem eco (nunca por argumento) e grava no `KeyringSecrets`; aceita `--email` para usar `guess_imap`.

- [ ] **Step 1: Write the failing tests**
  - `profile_add_list_rm_cycle` (config em diretório temporário via `--config-dir`).
  - `profile_add_rejects_password_in_args`: `--password` não existe → erro de uso do clap.
  - `verify_exit_code_is_1_when_eml_missing` e `0` quando íntegro.
  - `reindex_recovers_after_deleting_index_db`.
  - `status_prints_counts_per_folder`.
- [ ] **Step 2: Run** `cargo test -p mailbackup --test cli` — Expected: FAIL.
- [ ] **Step 3: Implement** os comandos como wrappers finos; `migrate` chama `archive` e depois `export` com o `TargetSpec` do perfil. Tratamento de `Ctrl+C`: sinalizar parada ao final do lote atual (flag atômica) e sair com código `2`.
- [ ] **Step 4: Run** o mesmo comando; depois `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -am "feat(cli): profile, archive, export, migrate, verify, reindex, status"`

### Task 8: Integração ponta a ponta com Dovecot e README

**Files:**
- Create: `tests/dovecot/docker-compose.yml`, `tests/dovecot/seed/` (mensagens de exemplo, incluindo sem `Message-ID`, anexo grande e pasta com acento), `crates/cli/tests/e2e_dovecot.rs` (`#[ignore]`), `README.md`

- [ ] **Step 1: Write the failing test** `e2e_archive_then_export_to_second_account`: arquivar `origem`, exportar para `destino` (duas contas no mesmo Dovecot), conferir contagem, flags e hashes por pasta; reexecutar `export` → nada novo.
- [ ] **Step 2: Run** `docker compose -f tests/dovecot/docker-compose.yml up -d` e `cargo test --workspace -- --ignored` — Expected: FAIL antes da implementação do compose/seed.
- [ ] **Step 3: Criar o compose/seed e o README** (instalação, `profile add`, exemplos de `archive`/`export`/`migrate`, formato do arquivo, aviso de que o arquivo é `.eml` + `index.db` e deve ser copiado com o app fechado).
- [ ] **Step 4: Run** o mesmo comando — Expected: PASS; `cargo clippy --workspace -- -D warnings` limpo.
- [ ] **Step 5: Commit** `git add -A && git commit -m "test: dovecot e2e and README"`

---

## Self-review

- **Cobertura da spec:** §3 estrutura → T1–T7; §4 formato/tabelas → T2; §5 arquivar/exportar/migrar/erros → T3, T5, T7; §6 testes → todas (proptest em T3, Dovecot em T4/T5/T8); §1 critério "sem senha em disco" → T6/T7; `status`/`reindex`/`verify` → T2/T7.
- **Consistência de tipos:** `Store`, `MailSource`, `MailTarget`, `RawMessage`, `Event` e `RunSummary` têm uma única definição (T1–T5), e os consumidores citam a assinatura.
- **Proporção:** o plano traz assinaturas, nomes de teste e asserções; os corpos ficam para quem implementa.
