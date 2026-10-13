//! `MailTarget` backed by a real IMAP server (`CREATE` + `APPEND`).
use crate::imap_conn::{Connector, Failure, ImapConnection, RetryPolicy, SleepFn, TlsConnector};
use crate::imap_source::to_wire_path;
use crate::target::MailTarget;
use crate::tls::TlsMode;
use crate::{Authenticator, Error, Result};
use imap::types::Flag;
use std::collections::HashMap;
use std::sync::Arc;

/// What is known about a destination folder after `ensure_folder`.
#[derive(Clone)]
struct FolderInfo {
    /// Name on the wire: modified UTF-7, destination delimiter.
    wire: String,
    /// The destination accepts new keywords (`PERMANENTFLAGS` contains `\*`).
    keywords: bool,
}

/// Appends messages to an IMAP account. Never reads or alters existing messages.
pub struct ImapTarget {
    conn: ImapConnection,
    id: String,
    /// `None` until discovered; `Some(None)` for a flat (no hierarchy) server.
    delimiter: Option<Option<String>>,
    folders: HashMap<String, FolderInfo>,
}

impl ImapTarget {
    /// Connect over TLS (certificate validated against the OS roots) and log in; same retry and
    /// authentication behavior as `ImapSource::connect`.
    pub fn connect(
        host: &str,
        port: u16,
        tls: TlsMode,
        auth: Box<dyn Authenticator + Send>,
    ) -> Result<ImapTarget> {
        let id = format!("imap://{}@{}:{}", auth.user(), host, port);
        let sleep: SleepFn = Arc::new(std::thread::sleep);
        Self::with_connector(
            Box::new(TlsConnector::new(host, port, tls)),
            auth,
            RetryPolicy::default(),
            sleep,
            id,
        )
    }

    pub(crate) fn with_connector(
        connector: Box<dyn Connector>,
        auth: Box<dyn Authenticator + Send>,
        policy: RetryPolicy,
        sleep: SleepFn,
        id: String,
    ) -> Result<ImapTarget> {
        let mut conn = ImapConnection::new(connector, auth, policy, sleep)?;
        conn.open()?;
        Ok(ImapTarget {
            conn,
            id,
            delimiter: None,
            folders: HashMap::new(),
        })
    }
}

/// Standard flags that survive an export, in `imap` form.
fn system_flag(f: &str) -> Option<Flag<'static>> {
    match f.to_ascii_lowercase().as_str() {
        "\\seen" => Some(Flag::Seen),
        "\\answered" => Some(Flag::Answered),
        "\\flagged" => Some(Flag::Flagged),
        "\\draft" => Some(Flag::Draft),
        _ => None,
    }
}

/// A flag-keyword is an IMAP atom (RFC 3501): printable ASCII without atom-specials.
fn is_keyword(f: &str) -> bool {
    !f.is_empty()
        && !f.starts_with('\\')
        && f.bytes()
            .all(|b| b.is_ascii_graphic() && !b"(){%*\"\\]".contains(&b))
}

/// Flags to send with `APPEND`.
///
/// Kept: `\Seen \Answered \Flagged \Draft`, and custom keywords when the destination accepts new
/// ones (`PERMANENTFLAGS` has `\*`; otherwise the server could reject the whole `APPEND`).
/// Dropped: `\Recent` (per-session, cannot be set by clients), `\Deleted` (restoring a backup must
/// not bring back messages that were pending expunge, nor hide them), unknown `\` flags and
/// anything that is not a safe atom.
fn append_flags(flags: &[String], keywords_ok: bool) -> Vec<Flag<'static>> {
    let mut out: Vec<Flag<'static>> = vec![];
    for f in flags {
        let flag = match system_flag(f) {
            Some(s) => s,
            None if keywords_ok && is_keyword(f) => Flag::Custom(f.clone().into()),
            None => continue,
        };
        if !out.contains(&flag) {
            out.push(flag);
        }
    }
    out
}

/// Names go on the wire inside a quoted string that `APPEND` does not escape.
fn check_wire_name(name: &str) -> Result<()> {
    if name.contains(['"', '\\', '\r', '\n']) {
        return Err(Error::Imap(format!(
            "folder name {name:?} contains characters that cannot be sent safely"
        )));
    }
    Ok(())
}

fn already_exists(no: &imap::error::No) -> bool {
    format!("{:?} {}", no.code, no.information)
        .to_ascii_lowercase()
        .replace(" ", "")
        .contains("alreadyexists")
}

impl ImapTarget {
    /// The destination's hierarchy delimiter (`LIST "" ""`); `None` for a flat server.
    fn delimiter(&mut self) -> Result<Option<String>> {
        if self.delimiter.is_none() {
            let names = self.conn.run(|s| s.list(Some(""), None))?;
            let d = names
                .iter()
                .find_map(|n| n.delimiter().map(str::to_string))
                .filter(|d| !d.is_empty());
            self.delimiter = Some(d);
        }
        Ok(self.delimiter.clone().flatten())
    }

    fn folder_info(&mut self, folder: &str) -> Result<FolderInfo> {
        if !self.folders.contains_key(folder) {
            self.ensure_folder(folder)?;
        }
        Ok(self.folders[folder].clone())
    }
}

impl MailTarget for ImapTarget {
    fn id(&self) -> String {
        self.id.clone()
    }

    /// Creates the folder and any missing parents (an existing mailbox is not an error), then
    /// opens it once to learn whether the destination accepts new keywords. The canonical "/"
    /// path is converted to the destination's delimiter and modified UTF-7. (A literal "/"
    /// inside a component cannot be represented on a server whose delimiter is not "/".)
    fn ensure_folder(&mut self, folder: &str) -> Result<()> {
        if self.folders.contains_key(folder) {
            return Ok(());
        }
        let delimiter = self.delimiter()?;
        let sep = delimiter.as_deref().unwrap_or("/");
        let comps: Vec<&str> = folder.split('/').filter(|c| !c.is_empty()).collect();
        if comps.is_empty() {
            return Err(Error::Imap("empty folder name".into()));
        }
        for i in 1..=comps.len() {
            let leaf = i == comps.len();
            // A flat server has no parents to create.
            if !leaf && delimiter.is_none() {
                continue;
            }
            let wire = to_wire_path(&comps[..i].join("/"), sep);
            check_wire_name(&wire)?;
            if wire == "INBOX" {
                continue; // always exists
            }
            self.conn.run(|s| match s.create(&wire) {
                Err(imap::Error::No(no)) if already_exists(&no) => Ok(()),
                // A parent that cannot be created on its own (some servers refuse) is not fatal:
                // creating the leaf, or the SELECT below, reports the real problem.
                Err(imap::Error::No(_)) if !leaf => Ok(()),
                r => r,
            })?;
        }
        let wire = to_wire_path(&comps.join("/"), sep);
        // SELECT, not EXAMINE: a read-only view reports no usable PERMANENTFLAGS.
        let mb = self.conn.run(|s| s.select(&wire))?;
        let keywords = mb.permanent_flags.contains(&Flag::MayCreate);
        self.folders
            .insert(folder.to_string(), FolderInfo { wire, keywords });
        Ok(())
    }

    /// Sends the message with a single `APPEND`, never re-sent: if the connection drops while it
    /// is in flight the server may or may not have stored it, so the message is reported as
    /// failed (the next export run retries it; the possible cost is one duplicate at the
    /// destination).
    fn put(
        &mut self,
        folder: &str,
        bytes: &[u8],
        flags: &[String],
        internal_date: Option<i64>,
    ) -> Result<()> {
        let info = self.folder_info(folder)?;
        let flags = append_flags(flags, info.keywords);
        let date = internal_date
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|d| d.fixed_offset());
        let sent = self.conn.run_once(|s| {
            let mut cmd = s.append(&info.wire, bytes);
            cmd.flags(flags);
            if let Some(d) = date {
                cmd.internal_date(d);
            }
            cmd.finish()
        });
        match sent {
            Ok(_) => Ok(()),
            Err(Failure::Transient(e)) => Err(Error::Connection(format!(
                "APPEND to {folder} interrupted, delivery unconfirmed (not retried): {e}"
            ))),
            Err(Failure::Fatal(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::PasswordAuth;
    use crate::imap_conn::test_support::*;

    fn target(server: &Arc<Server>) -> ImapTarget {
        let (sleep, _) = no_sleep();
        ImapTarget::with_connector(
            Box::new(MockConnector(server.clone())),
            Box::new(PasswordAuth::new("alice", "hunter2")),
            RetryPolicy::default(),
            sleep,
            "imap://alice@mock:993".into(),
        )
        .unwrap()
    }

    fn cmds(server: &Arc<Server>) -> Vec<String> {
        server.commands().into_iter().map(|(_, c)| c).collect()
    }

    /// A destination with the given delimiter that accepts everything.
    /// `perm` is the text of the PERMANENTFLAGS response code.
    fn dest(delim: &'static str, perm: &'static str) -> Arc<Server> {
        Server::new(move |_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK logged in\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text(format!(
                    "* LIST (\\Noselect) \"{delim}\" \"\"\r\n{{tag}} OK done\r\n"
                ))
            } else if cmd.starts_with("SELECT") {
                Reply::Text(format!(
                    "* 0 EXISTS\r\n* OK [PERMANENTFLAGS ({perm})] ok\r\n* OK [UIDVALIDITY 1] v\r\n* OK [UIDNEXT 1] n\r\n{{tag}} OK [READ-WRITE] ok\r\n"
                ))
            } else if cmd.starts_with("APPEND") {
                Reply::Text("+ Ready\r\n".into())
            } else if cmd == "LITERAL" {
                Reply::Text("{tag} OK [APPENDUID 1 2] done\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        })
    }

    #[test]
    fn ensure_folder_discovers_delimiter_creates_parents_and_encodes_names() {
        let server = dest(".", "\\Seen \\*");
        let mut t = target(&server);
        t.ensure_folder("Pai/Caixa de ção").unwrap();
        let c = cmds(&server);
        let pos = |want: &str| {
            c.iter()
                .position(|x| x == want)
                .unwrap_or_else(|| panic!("{want} not in {c:?}"))
        };
        assert!(c.contains(&"LIST \"\" \"\"".to_string()), "{c:?}");
        assert!(pos("LIST \"\" \"\"") < pos("CREATE \"Pai\""));
        assert!(pos("CREATE \"Pai\"") < pos("CREATE \"Pai.Caixa de &AOcA4w-o\""));
        assert!(
            pos("CREATE \"Pai.Caixa de &AOcA4w-o\"") < pos("SELECT \"Pai.Caixa de &AOcA4w-o\"")
        );
        // Cached: nothing more is sent the second time.
        let before = c.len();
        t.ensure_folder("Pai/Caixa de ção").unwrap();
        assert_eq!(cmds(&server).len(), before);
    }

    #[test]
    fn ensure_folder_ignores_already_exists_and_never_creates_inbox() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text("* LIST (\\Noselect) \"/\" \"\"\r\n{tag} OK\r\n".into())
            } else if cmd.starts_with("CREATE") {
                Reply::Text("{tag} NO [ALREADYEXISTS] Mailbox already exists\r\n".into())
            } else if cmd.starts_with("SELECT") {
                Reply::Text("* OK [PERMANENTFLAGS ()] ok\r\n{tag} OK [READ-WRITE] ok\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let mut t = target(&server);
        t.ensure_folder("A/B").unwrap();
        t.ensure_folder("inbox").unwrap();
        t.ensure_folder("INBOX/Sub").unwrap();
        let c = cmds(&server);
        assert!(c.contains(&"SELECT \"INBOX\"".to_string()), "{c:?}");
        assert!(!c.contains(&"CREATE \"INBOX\"".to_string()), "{c:?}");
        assert!(c.contains(&"CREATE \"INBOX/Sub\"".to_string()), "{c:?}");
    }

    #[test]
    fn nonexistent_parent_error_is_not_mistaken_for_already_exists() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text("* LIST (\\Noselect) \"/\" \"\"\r\n{tag} OK\r\n".into())
            } else if cmd.starts_with("CREATE") {
                Reply::Text("{tag} NO [NONEXISTENT] Parent mailbox does not exist\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let mut t = target(&server);
        let err = t.ensure_folder("Novo").unwrap_err();
        assert!(matches!(err, Error::Imap(_)), "{err:?}");
        assert!(!cmds(&server).iter().any(|c| c.starts_with("SELECT")));
    }

    #[test]
    fn ensure_folder_reports_real_create_failures() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text("* LIST (\\Noselect) \"/\" \"\"\r\n{tag} OK\r\n".into())
            } else if cmd.starts_with("CREATE") {
                Reply::Text("{tag} NO [NOPERM] Permission denied\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let mut t = target(&server);
        let err = t.ensure_folder("Novo").unwrap_err();
        assert!(matches!(err, Error::Imap(_)), "{err:?}");
        assert!(!cmds(&server).iter().any(|c| c.starts_with("SELECT")));
    }

    #[test]
    fn put_appends_with_flags_and_internal_date_in_wire_format() {
        let server = dest("/", "\\Seen \\Answered \\Flagged \\Deleted \\Draft \\*");
        let mut t = target(&server);
        t.ensure_folder("Pai/Filho").unwrap();
        let flags: Vec<String> = [
            "\\Seen",
            "\\flagged",
            "\\Recent",
            "\\Deleted",
            "$Label",
            "\\Draft",
            "\\Seen",
            "bad flag",
            "x(y",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        t.put(
            "Pai/Filho",
            b"Subject: hi\r\n\r\nbody\r\n",
            &flags,
            Some(1_700_000_000),
        )
        .unwrap();
        let c = cmds(&server);
        let append = c.iter().find(|c| c.starts_with("APPEND")).unwrap();
        assert_eq!(
            append,
            "APPEND \"Pai/Filho\" (\\Seen \\Flagged $Label \\Draft) \"14-Nov-2023 22:13:20 +0000\" {21}"
        );
        assert_eq!(
            server.literals(),
            vec![(1, b"Subject: hi\r\n\r\nbody\r\n".to_vec())]
        );
    }

    #[test]
    fn put_without_date_omits_it_and_empty_flags_are_valid() {
        let server = dest("/", "\\*");
        let mut t = target(&server);
        t.put("INBOX", b"x", &[], None).unwrap();
        let c = cmds(&server);
        assert!(c.contains(&"APPEND \"INBOX\" () {1}".to_string()), "{c:?}");
    }

    #[test]
    fn keywords_are_dropped_unless_destination_allows_new_ones() {
        let server = dest("/", "\\Seen \\Flagged");
        let mut t = target(&server);
        let flags = vec![
            "\\Seen".to_string(),
            "$Label".to_string(),
            "\\Flagged".to_string(),
        ];
        t.put("INBOX", b"x", &flags, None).unwrap();
        let c = cmds(&server);
        assert!(
            c.contains(&"APPEND \"INBOX\" (\\Seen \\Flagged) {1}".to_string()),
            "{c:?}"
        );
    }

    #[test]
    fn put_creates_the_folder_on_demand() {
        let server = dest("/", "\\*");
        let mut t = target(&server);
        t.put("Novo/Sub", b"x", &[], None).unwrap();
        let c = cmds(&server);
        assert!(
            c.contains(&"CREATE \"Novo\"".to_string())
                && c.contains(&"CREATE \"Novo/Sub\"".to_string()),
            "{c:?}"
        );
    }

    #[test]
    fn folder_names_that_could_break_the_command_are_rejected() {
        let server = dest("/", "\\*");
        let mut t = target(&server);
        for bad in ["a\"b", "a\\b"] {
            assert!(t.put(bad, b"x", &[], None).is_err(), "{bad:?}");
        }
        assert!(!cmds(&server).iter().any(|c| c.starts_with("APPEND")));
        // Control characters are modified-UTF-7 encoded, so they never reach the wire raw.
        t.put("a\r\nb", b"x", &[], None).unwrap();
        let c = cmds(&server);
        assert!(
            c.contains(&"APPEND \"a&AA0ACg-b\" () {1}".to_string()),
            "{c:?}"
        );
    }

    #[test]
    fn append_is_not_retried_after_a_dropped_connection_and_next_put_reconnects() {
        let appends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let a2 = appends.clone();
        let server = Server::new(move |conn, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text("* LIST (\\Noselect) \"/\" \"\"\r\n{tag} OK\r\n".into())
            } else if cmd.starts_with("SELECT") {
                Reply::Text("* OK [PERMANENTFLAGS (\\*)] ok\r\n{tag} OK [READ-WRITE] ok\r\n".into())
            } else if cmd.starts_with("APPEND") {
                a2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Reply::Text("+ Ready\r\n".into())
            } else if cmd == "LITERAL" {
                if conn == 1 {
                    // The server may or may not have committed: the connection just dies.
                    Reply::Drop(String::new())
                } else {
                    Reply::Text("{tag} OK done\r\n".into())
                }
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let mut t = target(&server);
        t.ensure_folder("INBOX").unwrap();
        let err = t.put("INBOX", b"first", &[], None).unwrap_err();
        assert!(
            matches!(&err, Error::Connection(m) if m.contains("not retried")),
            "{err:?}"
        );
        assert_eq!(
            appends.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "APPEND must not be re-sent"
        );
        assert_eq!(server.connection_count(), 1);
        // The next message reconnects lazily and goes through.
        t.put("INBOX", b"second", &[], None).unwrap();
        assert_eq!(server.connection_count(), 2);
        assert_eq!(appends.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn append_rejected_by_server_is_a_plain_error() {
        let server = Server::new(|_, cmd| {
            if cmd.starts_with("LOGIN") {
                Reply::Text("{tag} OK\r\n".into())
            } else if cmd.starts_with("LIST") {
                Reply::Text("* LIST (\\Noselect) \"/\" \"\"\r\n{tag} OK\r\n".into())
            } else if cmd.starts_with("SELECT") {
                Reply::Text("* OK [PERMANENTFLAGS (\\*)] ok\r\n{tag} OK [READ-WRITE] ok\r\n".into())
            } else if cmd.starts_with("APPEND") {
                Reply::Text("{tag} NO [OVERQUOTA] Quota exceeded\r\n".into())
            } else {
                Reply::Text("{tag} OK\r\n".into())
            }
        });
        let mut t = target(&server);
        assert!(t.put("INBOX", b"x", &[], None).is_err());
        assert_eq!(server.connection_count(), 1);
    }

    #[test]
    fn id_names_user_host_and_port() {
        let server = dest("/", "\\*");
        assert_eq!(target(&server).id(), "imap://alice@mock:993");
    }
}
