//! Reusable IMAP connection: TLS connect, login, transparent reconnect with exponential backoff
//! and re-EXAMINE of the current folder. Shared by `ImapSource` (and later `ImapTarget`).
use crate::tls::TlsMode;
use crate::{Authenticator, Error, Result};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub(crate) type Session = imap::Session<imap::Connection>;

/// Retry schedule: `attempts` tries in total, sleeping `base * 2^(n-1)` after failed try `n`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    pub attempts: u32,
    pub base: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_secs(1),
        }
    }
}

pub(crate) enum Failure {
    /// Network-level problem: worth another try after reconnecting.
    Transient(Error),
    /// Do not retry (authentication, protocol, TLS policy...).
    Fatal(Error),
}

pub(crate) type SleepFn = Arc<dyn Fn(Duration) + Send + Sync>;

/// Run `op` until it succeeds, fails fatally, or `policy.attempts` is exhausted.
pub(crate) fn retry<T>(
    policy: RetryPolicy,
    sleep: &dyn Fn(Duration),
    mut op: impl FnMut() -> std::result::Result<T, Failure>,
) -> Result<T> {
    let attempts = policy.attempts.max(1);
    let mut n = 0;
    loop {
        n += 1;
        match op() {
            Ok(v) => return Ok(v),
            Err(Failure::Fatal(e)) => return Err(e),
            Err(Failure::Transient(e)) => {
                if n >= attempts {
                    return Err(e);
                }
                sleep(policy.base * 2u32.saturating_pow(n - 1));
            }
        }
    }
}

/// Creates a fresh, unauthenticated client (greeting already read).
pub(crate) trait Connector: Send {
    fn connect(&mut self) -> imap::Result<imap::Client<imap::Connection>>;
}

/// Production connector: rustls with OS root certificates; certificate validation is always on
/// (`danger_skip_tls_verify` is never called) and there is no plaintext mode.
pub(crate) struct TlsConnector {
    host: String,
    port: u16,
    mode: TlsMode,
}

impl TlsConnector {
    pub(crate) fn new(host: &str, port: u16, mode: TlsMode) -> Self {
        Self {
            host: host.to_string(),
            port,
            mode,
        }
    }
}

impl Connector for TlsConnector {
    fn connect(&mut self) -> imap::Result<imap::Client<imap::Connection>> {
        let mode = match self.mode {
            TlsMode::Implicit => imap::ConnectionMode::Tls,
            TlsMode::StartTls => imap::ConnectionMode::StartTls,
        };
        imap::ClientBuilder::new(self.host.as_str(), self.port)
            .mode(mode)
            .tls_kind(imap::TlsKind::Rust)
            .connect()
    }
}

pub(crate) struct ImapConnection {
    connector: Box<dyn Connector>,
    auth: Box<dyn Authenticator + Send>,
    policy: RetryPolicy,
    sleep: SleepFn,
    session: Option<Session>,
    /// Wire (modified UTF-7) name and UIDVALIDITY of the mailbox to re-open after a reconnect.
    selected: Option<(String, u32)>,
}

impl fmt::Debug for ImapConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConnection")
            .field("user", &self.auth.user())
            .field("connected", &self.session.is_some())
            .field("selected", &self.selected)
            .finish()
    }
}

/// Network-level errors worth a reconnect.
fn is_transient(e: &imap::Error) -> bool {
    matches!(
        e,
        imap::Error::Io(_)
            | imap::Error::ConnectionLost
            | imap::Error::Bye(_)
            | imap::Error::RustlsHandshake(_)
            | imap::Error::TagMismatch(_)
    )
}

fn classify(e: imap::Error) -> Failure {
    let err = Error::Imap(e.to_string());
    if is_transient(&e) {
        Failure::Transient(err)
    } else {
        Failure::Fatal(err)
    }
}

impl ImapConnection {
    /// Does not connect yet. The password is requested from `auth` at every (re)login and not
    /// stored here.
    pub(crate) fn new(
        connector: Box<dyn Connector>,
        auth: Box<dyn Authenticator + Send>,
        policy: RetryPolicy,
        sleep: SleepFn,
    ) -> Result<Self> {
        Ok(Self {
            connector,
            auth,
            policy,
            sleep,
            session: None,
            selected: None,
        })
    }

    /// Connect eagerly (with retries) so bad credentials / hosts surface at construction.
    pub(crate) fn open(&mut self) -> Result<()> {
        let sleep = self.sleep.clone();
        let policy = self.policy;
        retry(policy, &*sleep, || self.ensure_session().map(|_| ()))
    }

    /// Remember the opened folder and its UIDVALIDITY. After a reconnect the folder is
    /// re-opened and a changed UIDVALIDITY is a fatal error (UIDs would map to other messages).
    pub(crate) fn set_selected(&mut self, selected: Option<(String, u32)>) {
        self.selected = selected;
    }

    /// Run an IDEMPOTENT operation (search, fetch, list, ...). On a network error the connection
    /// is re-established (re-opening the current folder) and `f` is run again, up to the retry
    /// policy. Server-side rejections are returned at once.
    ///
    /// Do NOT use for commands with side effects (APPEND, STORE...): the server may have
    /// committed before the connection dropped, and a retry would repeat the effect. Use
    /// [`Self::run_once`] for those.
    pub(crate) fn run<R>(
        &mut self,
        mut f: impl FnMut(&mut Session) -> imap::Result<R>,
    ) -> Result<R> {
        let sleep = self.sleep.clone();
        let policy = self.policy;
        retry(policy, &*sleep, || {
            let session = self.ensure_session()?;
            match f(session) {
                Ok(v) => Ok(v),
                Err(e) => {
                    let failure = classify(e);
                    if matches!(failure, Failure::Transient(_)) {
                        self.session = None;
                    }
                    Err(failure)
                }
            }
        })
    }

    /// Run `f` exactly once. Connecting (before `f`) is retried as for `run`, but if `f` itself
    /// hits a network error it is NOT re-issued: the broken session is dropped (the next call
    /// reconnects lazily) and the error is returned classified, so the caller can decide
    /// whether the operation may have taken effect (`Failure::Transient`) or was rejected
    /// (`Failure::Fatal`).
    pub(crate) fn run_once<R>(
        &mut self,
        f: impl FnOnce(&mut Session) -> imap::Result<R>,
    ) -> std::result::Result<R, Failure> {
        let sleep = self.sleep.clone();
        let policy = self.policy;
        let mut fatal = false;
        let connected = retry(policy, &*sleep, || {
            let r = self.ensure_session().map(|_| ());
            if matches!(r, Err(Failure::Fatal(_))) {
                fatal = true;
            }
            r
        });
        if let Err(e) = connected {
            return Err(if fatal {
                Failure::Fatal(e)
            } else {
                Failure::Transient(e)
            });
        }
        let session = self.session.as_mut().expect("connected");
        match f(session) {
            Ok(v) => Ok(v),
            Err(e) => {
                let failure = classify(e);
                if matches!(failure, Failure::Transient(_)) {
                    self.session = None;
                }
                Err(failure)
            }
        }
    }

    fn ensure_session(&mut self) -> std::result::Result<&mut Session, Failure> {
        if self.session.is_none() {
            let client = self.connector.connect().map_err(|e| match e {
                // Never fall back to plaintext.
                imap::Error::StartTlsNotAvailable | imap::Error::TlsNotConfigured => {
                    Failure::Fatal(Error::Imap(e.to_string()))
                }
                other => classify(other),
            })?;
            let password = self.auth.password().map_err(Failure::Fatal)?;
            let login = client.login(self.auth.user(), &password);
            drop(password);
            let mut session = match login {
                Ok(s) => s,
                Err((e, _client)) => {
                    return Err(match e {
                        imap::Error::No(_) | imap::Error::Bad(_) => {
                            Failure::Fatal(Error::Auth(format!("login rejected by server: {e}")))
                        }
                        // Display of Validate would echo the offending character.
                        imap::Error::Validate(_) => Failure::Fatal(Error::Auth(
                            "credentials contain characters IMAP cannot send".into(),
                        )),
                        other => classify(other),
                    });
                }
            };
            if let Some((name, uidvalidity)) = &self.selected {
                // Read-only: a backup must never change server state (e.g. \Recent).
                let mb = session.examine(name).map_err(classify)?;
                if mb.uid_validity != Some(*uidvalidity) {
                    return Err(Failure::Fatal(Error::Imap(format!(
                        "UIDVALIDITY of {name} changed during the run (was {uidvalidity}, now {:?}); the next run will reconcile",
                        mb.uid_validity
                    ))));
                }
            }
            self.session = Some(session);
        }
        Ok(self.session.as_mut().expect("session just set"))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A scripted in-memory IMAP server driving the real `imap` client over a fake stream.
    use super::*;
    use imap::extensions::idle::SetReadTimeout;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::sync::Mutex;

    pub(crate) enum Reply {
        /// Untagged lines plus the tagged completion; `{tag}` is substituted.
        Text(String),
        /// Send this text and then close the connection (no tagged completion).
        Drop(String),
    }

    pub(crate) type Handler = dyn FnMut(usize, &str) -> Reply + Send;

    pub(crate) struct Server {
        handler: Mutex<Box<Handler>>,
        connections: Mutex<usize>,
        log: Mutex<Vec<(usize, String)>>,
        literals: Mutex<Vec<(usize, Vec<u8>)>>,
    }

    impl Server {
        pub(crate) fn new(h: impl FnMut(usize, &str) -> Reply + Send + 'static) -> Arc<Server> {
            Arc::new(Server {
                handler: Mutex::new(Box::new(h)),
                connections: Mutex::new(0),
                log: Mutex::new(vec![]),
                literals: Mutex::new(vec![]),
            })
        }
        /// Message bodies sent as `{N}` literals (connection index, bytes). The handler is
        /// called with the pseudo command `LITERAL` once a literal has been received, and its
        /// reply may use `{tag}` of the command that announced it.
        pub(crate) fn literals(&self) -> Vec<(usize, Vec<u8>)> {
            self.literals.lock().unwrap().clone()
        }
        /// Commands seen (connection index starting at 1, command text without tag).
        pub(crate) fn commands(&self) -> Vec<(usize, String)> {
            self.log.lock().unwrap().clone()
        }
        pub(crate) fn connection_count(&self) -> usize {
            *self.connections.lock().unwrap()
        }
    }

    struct MockStream {
        server: Arc<Server>,
        conn: usize,
        written: Vec<u8>,
        out: VecDeque<u8>,
        closed: bool,
        literal: Option<(String, usize)>,
    }

    impl Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut n = 0;
            while n < buf.len() {
                match self.out.pop_front() {
                    Some(b) => {
                        buf[n] = b;
                        n += 1;
                    }
                    None => break,
                }
            }
            Ok(n) // 0 == EOF once the scripted bytes are exhausted
        }
    }

    impl MockStream {
        fn respond(&mut self, tag: &str, cmd: &str) {
            let reply = (self.server.handler.lock().unwrap())(self.conn, cmd);
            let (text, close) = match reply {
                Reply::Text(t) => (t, false),
                Reply::Drop(t) => (t, true),
            };
            self.out.extend(text.replace("{tag}", tag).bytes());
            if close {
                self.closed = true;
            }
        }
    }

    /// `{N}` at the end of a command announces a literal of N bytes.
    fn literal_len(cmd: &str) -> Option<usize> {
        let inner = cmd.strip_suffix('}')?;
        inner.rsplit_once('{')?.1.parse().ok()
    }

    impl Write for MockStream {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.closed {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
            }
            self.written.extend_from_slice(data);
            loop {
                if let Some((tag, n)) = self.literal.clone() {
                    if self.written.len() < n + 2 {
                        break;
                    }
                    let body: Vec<u8> = self.written.drain(..n).collect();
                    self.written.drain(..2);
                    self.server.literals.lock().unwrap().push((self.conn, body));
                    self.literal = None;
                    self.respond(&tag, "LITERAL");
                    continue;
                }
                let Some(pos) = self.written.windows(2).position(|w| w == b"\r\n") else {
                    break;
                };
                let line: Vec<u8> = self.written.drain(..pos + 2).collect();
                let line = String::from_utf8_lossy(&line[..pos]).to_string();
                let (tag, cmd) = line.split_once(' ').unwrap_or((&line, ""));
                self.server
                    .log
                    .lock()
                    .unwrap()
                    .push((self.conn, cmd.to_string()));
                self.respond(tag, cmd);
                if let Some(n) = literal_len(cmd) {
                    self.literal = Some((tag.to_string(), n));
                }
            }
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SetReadTimeout for MockStream {
        fn set_read_timeout(&mut self, _: Option<Duration>) -> imap::Result<()> {
            Ok(())
        }
    }

    pub(crate) struct MockConnector(pub(crate) Arc<Server>);

    impl Connector for MockConnector {
        fn connect(&mut self) -> imap::Result<imap::Client<imap::Connection>> {
            let conn = {
                let mut c = self.0.connections.lock().unwrap();
                *c += 1;
                *c
            };
            let stream = MockStream {
                server: self.0.clone(),
                conn,
                written: vec![],
                out: VecDeque::from(b"* OK mock ready\r\n".to_vec()),
                closed: false,
                literal: None,
            };
            let mut client = imap::Client::new(Box::new(stream) as imap::Connection);
            client.read_greeting()?;
            Ok(client)
        }
    }

    pub(crate) fn no_sleep() -> (SleepFn, Arc<Mutex<Vec<Duration>>>) {
        let rec = Arc::new(Mutex::new(vec![]));
        let r2 = rec.clone();
        (Arc::new(move |d| r2.lock().unwrap().push(d)), rec)
    }

    pub(crate) fn conn_for(server: &Arc<Server>) -> (ImapConnection, Arc<Mutex<Vec<Duration>>>) {
        let (sleep, rec) = no_sleep();
        let auth = crate::auth::PasswordAuth::new("alice", "hunter2");
        let c = ImapConnection::new(
            Box::new(MockConnector(server.clone())),
            Box::new(auth),
            RetryPolicy::default(),
            sleep,
        )
        .unwrap();
        (c, rec)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retry_succeeds_after_transient_failures_with_exponential_backoff() {
        let slept = std::cell::RefCell::new(vec![]);
        let tries = Cell::new(0);
        let r = retry(
            RetryPolicy {
                attempts: 3,
                base: Duration::from_millis(100),
            },
            &|d| slept.borrow_mut().push(d),
            || {
                tries.set(tries.get() + 1);
                if tries.get() < 3 {
                    Err(Failure::Transient(Error::Imap("net".into())))
                } else {
                    Ok(42)
                }
            },
        );
        assert_eq!(r.unwrap(), 42);
        assert_eq!(tries.get(), 3);
        assert_eq!(
            *slept.borrow(),
            vec![Duration::from_millis(100), Duration::from_millis(200)]
        );
    }

    #[test]
    fn retry_gives_up_after_policy_attempts() {
        let tries = Cell::new(0);
        let r: Result<()> = retry(RetryPolicy::default(), &|_| {}, || {
            tries.set(tries.get() + 1);
            Err(Failure::Transient(Error::Imap("net".into())))
        });
        assert!(matches!(r, Err(Error::Imap(_))));
        assert_eq!(tries.get(), 3);
    }

    #[test]
    fn retry_does_not_repeat_fatal_errors() {
        let tries = Cell::new(0);
        let r: Result<()> = retry(RetryPolicy::default(), &|_| {}, || {
            tries.set(tries.get() + 1);
            Err(Failure::Fatal(Error::Auth("no".into())))
        });
        assert!(matches!(r, Err(Error::Auth(_))));
        assert_eq!(tries.get(), 1);
    }

    #[test]
    fn auth_failure_is_not_retried() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} NO [AUTHENTICATIONFAILED] Authentication failed.\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let (mut c, slept) = conn_for(&server);
        let err = c.open().unwrap_err();
        assert!(matches!(err, Error::Auth(_)), "{err:?}");
        assert_eq!(server.connection_count(), 1, "exactly one attempt");
        assert_eq!(
            server
                .commands()
                .iter()
                .filter(|(_, c)| c.starts_with("LOGIN"))
                .count(),
            1
        );
        assert!(slept.lock().unwrap().is_empty());
        assert!(!err.to_string().contains("hunter2"));
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn reconnects_and_reselects_after_connection_drop() {
        let server = Server::new(|conn, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK logged in\r\n".into())
            } else if cmd.starts_with("EXAMINE") {
                Reply::Text(
                    "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n* OK [UIDNEXT 4] ok\r\n{tag} OK [READ-ONLY] done\r\n"
                        .into(),
                )
            } else if cmd.starts_with("NOOP") && conn == 1 {
                Reply::Drop(String::new()) // connection dies on the first NOOP
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let (mut c, slept) = conn_for(&server);
        c.open().unwrap();
        c.set_selected(Some(("INBOX".into(), 7)));
        c.run(|s| s.noop()).unwrap();
        assert_eq!(server.connection_count(), 2);
        let cmds: Vec<String> = server
            .commands()
            .into_iter()
            .filter(|(conn, _)| *conn == 2)
            .map(|(_, c)| c)
            .collect();
        assert!(cmds[0].starts_with("LOGIN"), "{cmds:?}");
        assert_eq!(cmds[1], "EXAMINE \"INBOX\"", "{cmds:?}");
        assert_eq!(cmds[2], "NOOP", "{cmds:?}");
        assert_eq!(slept.lock().unwrap().len(), 1);
    }

    #[test]
    fn gives_up_when_server_keeps_dropping() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else {
                Reply::Drop(String::new())
            }
        });
        let (mut c, slept) = conn_for(&server);
        let err = c.run(|s| s.noop()).unwrap_err();
        assert!(matches!(err, Error::Imap(_)), "{err:?}");
        assert_eq!(server.connection_count(), 3);
        assert_eq!(slept.lock().unwrap().len(), 2);
    }

    #[test]
    fn server_no_is_not_retried() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else {
                Reply::Text("{tag} NO nope\r\n".into())
            }
        });
        let (mut c, _) = conn_for(&server);
        assert!(matches!(c.run(|s| s.noop()), Err(Error::Imap(_))));
        assert_eq!(server.connection_count(), 1);
    }

    fn examine_server(second_uidvalidity: u32) -> Arc<Server> {
        Server::new(move |conn, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("EXAMINE") {
                let uv = if conn == 1 { 7 } else { second_uidvalidity };
                Reply::Text(format!(
                    "* OK [UIDVALIDITY {uv}] v\r\n* OK [UIDNEXT 4] n\r\n{{tag}} OK [READ-ONLY] ok\r\n"
                ))
            } else if cmd.starts_with("NOOP") && conn == 1 {
                Reply::Drop(String::new())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        })
    }

    #[test]
    fn uidvalidity_change_after_reconnect_is_fatal_and_stops() {
        let server = examine_server(8);
        let (mut c, slept) = conn_for(&server);
        c.open().unwrap();
        c.set_selected(Some(("INBOX".into(), 7)));
        let err = c.run(|s| s.noop()).unwrap_err();
        assert!(
            matches!(&err, Error::Imap(m) if m.contains("UIDVALIDITY")),
            "{err:?}"
        );
        assert_eq!(server.connection_count(), 2, "no further retries");
        assert_eq!(slept.lock().unwrap().len(), 1);
        let on2: Vec<String> = server
            .commands()
            .into_iter()
            .filter(|(n, _)| *n == 2)
            .map(|(_, c)| c)
            .collect();
        assert_eq!(
            on2.len(),
            2,
            "only LOGIN + EXAMINE on the new connection: {on2:?}"
        );
    }

    #[test]
    fn same_uidvalidity_after_reconnect_continues() {
        let server = examine_server(7);
        let (mut c, _) = conn_for(&server);
        c.open().unwrap();
        c.set_selected(Some(("INBOX".into(), 7)));
        c.run(|s| s.noop()).unwrap();
        assert_eq!(server.connection_count(), 2);
    }

    #[test]
    fn run_once_does_not_reissue_after_dropped_connection_but_reconnects_next_time() {
        let server = examine_server(7);
        let (mut c, _) = conn_for(&server);
        c.open().unwrap();
        let calls = Cell::new(0);
        let r = c.run_once(|s| {
            calls.set(calls.get() + 1);
            s.noop()
        });
        assert!(matches!(r, Err(Failure::Transient(_))));
        assert_eq!(calls.get(), 1);
        assert_eq!(
            server
                .commands()
                .iter()
                .filter(|(_, c)| c == "NOOP")
                .count(),
            1
        );
        // Next call reconnects lazily and works.
        assert!(c.run_once(|s| s.noop()).is_ok());
        assert_eq!(server.connection_count(), 2);
    }

    #[test]
    fn run_reissues_after_dropped_connection() {
        let server = examine_server(7);
        let (mut c, _) = conn_for(&server);
        c.open().unwrap();
        let calls = Cell::new(0);
        c.run(|s| {
            calls.set(calls.get() + 1);
            s.noop()
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn password_is_requested_again_on_every_login() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counting(Arc<AtomicUsize>);
        impl Authenticator for Counting {
            fn user(&self) -> &str {
                "alice"
            }
            fn password(&self) -> Result<String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok("pw".into())
            }
        }
        let server = examine_server(7);
        let asked = Arc::new(AtomicUsize::new(0));
        let (sleep, _) = no_sleep();
        let mut c = ImapConnection::new(
            Box::new(MockConnector(server.clone())),
            Box::new(Counting(asked.clone())),
            RetryPolicy::default(),
            sleep,
        )
        .unwrap();
        c.open().unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        c.run(|s| s.noop()).unwrap(); // drops on conn 1, reconnects
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }
}
