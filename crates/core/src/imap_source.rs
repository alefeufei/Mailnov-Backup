//! `MailSource` backed by a real IMAP server.
use crate::imap_conn::{Connector, ImapConnection, RetryPolicy, SleepFn, TlsConnector};
use crate::tls::TlsMode;
use crate::{Authenticator, Error, FolderState, MailSource, RawMessage, Result};
use imap_proto::NameAttribute;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// UIDs per command for header/flag lookups.
const LIGHT_CHUNK: usize = 1000;
/// UIDs per `UID FETCH ... BODY.PEEK[]` command (full bodies are large).
const FETCH_CHUNK: usize = 100;

/// Read-only IMAP mailbox: never sets `\Seen` (`BODY.PEEK`) and opens folders with `EXAMINE`.
pub struct ImapSource {
    conn: ImapConnection,
    /// Canonical folder path (components joined by "/") -> name exactly as the server sent it
    /// (modified UTF-7, server delimiter).
    wire_names: HashMap<String, String>,
    /// Hierarchy delimiter learned from the last `LIST`, used for names that were never listed.
    delimiter: Option<String>,
}

impl std::fmt::Debug for ImapSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapSource")
            .field("conn", &self.conn)
            .finish()
    }
}

impl ImapSource {
    /// Connect over TLS (certificate validated against the OS roots) and log in. A rejected login
    /// is `Error::Auth` after exactly one attempt; network errors are retried with backoff.
    pub fn connect(
        host: &str,
        port: u16,
        tls: TlsMode,
        auth: Box<dyn Authenticator + Send>,
    ) -> Result<ImapSource> {
        let sleep: SleepFn = Arc::new(std::thread::sleep);
        Self::with_connector(
            Box::new(TlsConnector::new(host, port, tls)),
            auth,
            RetryPolicy::default(),
            sleep,
        )
    }

    pub(crate) fn with_connector(
        connector: Box<dyn Connector>,
        auth: Box<dyn Authenticator + Send>,
        policy: RetryPolicy,
        sleep: SleepFn,
    ) -> Result<ImapSource> {
        let mut conn = ImapConnection::new(connector, auth, policy, sleep)?;
        conn.open()?;
        Ok(ImapSource {
            conn,
            wire_names: HashMap::new(),
            delimiter: None,
        })
    }

    /// Server-side name of a canonical ("/"-separated) folder path.
    fn wire_name(&self, folder: &str) -> String {
        self.wire_names
            .get(folder)
            .cloned()
            .unwrap_or_else(|| to_wire_path(folder, self.delimiter.as_deref().unwrap_or("/")))
    }
}

/// Canonical path of a server mailbox name: split on the server delimiter, decode each
/// component from modified UTF-7 and join with "/". (A literal "/" inside a component of a
/// server whose delimiter is not "/" cannot be told apart from a level separator: known
/// limitation.)
fn to_canonical_path(wire: &str, delimiter: Option<&str>) -> String {
    match delimiter {
        Some(d) if !d.is_empty() => wire
            .split(d)
            .map(decode_folder_name)
            .collect::<Vec<_>>()
            .join("/"),
        _ => decode_folder_name(wire),
    }
}

/// Inverse of [`to_canonical_path`] for names the server never listed. `INBOX` (first
/// component, any case) is the one name with a fixed spelling.
pub(crate) fn to_wire_path(canonical: &str, delimiter: &str) -> String {
    canonical
        .split('/')
        .enumerate()
        .map(|(i, c)| {
            if i == 0 && c.eq_ignore_ascii_case("INBOX") {
                "INBOX".to_string()
            } else {
                encode_folder_name(c)
            }
        })
        .collect::<Vec<_>>()
        .join(delimiter)
}

/// Extract the Message-ID value (without angle brackets) from a header block.
fn parse_message_id(header: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(header);
    let mut lines = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .peekable();
    while let Some(line) = lines.next() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("message-id") {
                let mut value = v.trim().to_string();
                while let Some(next) = lines.peek() {
                    if next.starts_with(' ') || next.starts_with('\t') {
                        value.push_str(next.trim());
                        lines.next();
                    } else {
                        break;
                    }
                }
                let v = value
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim();
                return if v.is_empty() {
                    None
                } else {
                    Some(v.to_string())
                };
            }
        }
    }
    None
}

fn flag_strings(flags: &[imap::types::Flag<'_>]) -> Vec<String> {
    flags
        .iter()
        // \Recent is per-session state, not something worth archiving.
        .filter(|f| !matches!(f, imap::types::Flag::Recent))
        .map(|f| f.to_string())
        .collect()
}

impl MailSource for ImapSource {
    fn list_folders(&mut self) -> Result<Vec<String>> {
        let names = self.conn.run(|s| s.list(Some(""), Some("*")))?;
        let mut out = Vec::new();
        let mut wire = HashMap::new();
        let mut delimiter: Option<String> = None;
        for n in names.iter() {
            let skip = n.attributes().iter().any(|a| match a {
                NameAttribute::NoSelect => true,
                NameAttribute::Extension(x) => x.eq_ignore_ascii_case("\\NonExistent"),
                _ => false,
            });
            if skip {
                continue;
            }
            if delimiter.is_none() {
                delimiter = n.delimiter().map(str::to_string);
            }
            // The archive uses "/" regardless of the server's hierarchy delimiter.
            let canonical = to_canonical_path(n.name(), n.delimiter());
            wire.insert(canonical.clone(), n.name().to_string());
            out.push(canonical);
        }
        self.wire_names = wire;
        if delimiter.is_some() {
            self.delimiter = delimiter;
        }
        Ok(out)
    }

    fn select(&mut self, folder: &str) -> Result<FolderState> {
        let wire = self.wire_name(folder);
        // Forget the previous folder first: a reconnect during this EXAMINE must not re-open
        // it (it may be gone), and after a refused EXAMINE no folder is open anyway.
        self.conn.set_selected(None);
        let mb = self.conn.run(|s| s.examine(&wire))?;
        // UIDNEXT is optional (the engine never uses it); UIDVALIDITY is what makes UIDs usable.
        match mb.uid_validity {
            Some(uidvalidity) => {
                self.conn.set_selected(Some((wire, uidvalidity)));
                Ok(FolderState {
                    uidvalidity,
                    uidnext: mb.uid_next.unwrap_or(0),
                })
            }
            None => Err(Error::Imap(format!(
                "server did not report UIDVALIDITY for {folder}"
            ))),
        }
    }

    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>> {
        if last_uid == u32::MAX {
            return Ok(vec![]);
        }
        let query = search_query(last_uid, since);
        let found = self.conn.run(|s| s.uid_search(&query))?;
        Ok(filter_new_uids(found, last_uid))
    }

    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>> {
        let wanted: HashSet<u32> = uids.iter().copied().collect();
        let mut out: BTreeMap<u32, Option<String>> = BTreeMap::new();
        for chunk in chunk_uids(uids, LIGHT_CHUNK) {
            let set = format_uid_set(&chunk);
            let fetches = self
                .conn
                .run(|s| s.uid_fetch(&set, "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"))?;
            for f in fetches.iter() {
                let Some(uid) = f.uid else { continue };
                if !wanted.contains(&uid) {
                    continue;
                }
                let id = f.header().and_then(parse_message_id);
                let slot = out.entry(uid).or_insert(None);
                if id.is_some() {
                    *slot = id;
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>> {
        let wanted: HashSet<u32> = uids.iter().copied().collect();
        let mut out: BTreeMap<u32, RawMessage> = BTreeMap::new();
        for chunk in chunk_uids(uids, FETCH_CHUNK) {
            let set = format_uid_set(&chunk);
            let fetches = self
                .conn
                .run(|s| s.uid_fetch(&set, "(UID FLAGS INTERNALDATE BODY.PEEK[])"))?;
            for f in fetches.iter() {
                let Some(uid) = f.uid else { continue };
                if !wanted.contains(&uid) {
                    continue;
                }
                // Skip partial/flag-only updates; a UID with no body ends up absent and the
                // engine treats a still-listed UID as failed.
                let Some(body) = f.body() else { continue };
                out.insert(
                    uid,
                    RawMessage {
                        uid,
                        flags: flag_strings(f.flags()),
                        internal_date: f.internal_date().map(|d| d.timestamp()),
                        bytes: body.to_vec(),
                    },
                );
            }
        }
        Ok(out.into_values().collect())
    }

    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        let wanted: HashSet<u32> = uids.iter().copied().collect();
        let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for chunk in chunk_uids(uids, LIGHT_CHUNK) {
            let set = format_uid_set(&chunk);
            let fetches = self.conn.run(|s| s.uid_fetch(&set, "(UID FLAGS)"))?;
            for f in fetches.iter() {
                let Some(uid) = f.uid else { continue };
                if wanted.contains(&uid) {
                    out.insert(uid, flag_strings(f.flags()));
                }
            }
        }
        Ok(out.into_iter().collect())
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b',' => Some(63),
        _ => None,
    }
}

/// Modified UTF-7 (RFC 3501 section 5.1.3) to UTF-8. Returns the input unchanged if it is not
/// valid modified UTF-7.
pub fn decode_folder_name(modified_utf7: &str) -> String {
    decode_inner(modified_utf7).unwrap_or_else(|| modified_utf7.to_string())
}

fn decode_inner(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let end = after.find('-')?;
        let enc = &after[..end];
        if enc.is_empty() {
            out.push('&');
        } else {
            let mut bits: u32 = 0;
            let mut nbits = 0;
            let mut bytes: Vec<u8> = Vec::new();
            for c in enc.bytes() {
                bits = (bits << 6) | b64_val(c)?;
                nbits += 6;
                if nbits >= 8 {
                    nbits -= 8;
                    bytes.push((bits >> nbits) as u8);
                    bits &= (1 << nbits) - 1;
                }
            }
            // Leftover bits must be zero padding (fewer than 6 bits, i.e. less than one char).
            if nbits >= 6 || bits != 0 || !bytes.len().is_multiple_of(2) || bytes.is_empty() {
                return None;
            }
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect();
            out.push_str(&String::from_utf16(&units).ok()?);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn flush_b64(units: &mut Vec<u16>, out: &mut String) {
    if units.is_empty() {
        return;
    }
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_be_bytes()).collect();
    out.push('&');
    for chunk in bytes.chunks(3) {
        let mut v: u32 = 0;
        for (i, b) in chunk.iter().enumerate() {
            v |= (*b as u32) << (16 - 8 * i);
        }
        let n_chars = (chunk.len() * 8).div_ceil(6);
        for i in 0..n_chars {
            out.push(B64[((v >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out.push('-');
    units.clear();
}

/// UTF-8 to modified UTF-7.
pub fn encode_folder_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    for c in name.chars() {
        if ('\u{20}'..='\u{7e}').contains(&c) {
            flush_b64(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush_b64(&mut pending, &mut out);
    out
}

fn sorted_unique(uids: &[u32]) -> Vec<u32> {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// Compress UIDs into an IMAP sequence set (`1:3,7`). Input is sorted and de-duplicated first.
pub(crate) fn format_uid_set(uids: &[u32]) -> String {
    let v = sorted_unique(uids);
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        i += 1;
    }
    parts.join(",")
}

/// Sort, de-duplicate and split into chunks of at most `size` UIDs.
pub(crate) fn chunk_uids(uids: &[u32], size: usize) -> Vec<Vec<u32>> {
    sorted_unique(uids)
        .chunks(size.max(1))
        .map(<[u32]>::to_vec)
        .collect()
}

/// `dd-Mon-yyyy` (UTC) for IMAP `SINCE`.
pub(crate) fn imap_date(unix: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Days-to-civil (Howard Hinnant).
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}-{}-{year:04}", MONTHS[(month - 1) as usize])
}

pub(crate) fn search_query(last_uid: u32, since: Option<i64>) -> String {
    let mut q = format!("UID {}:*", u64::from(last_uid) + 1);
    if let Some(ts) = since {
        q.push_str(" SINCE ");
        q.push_str(&imap_date(ts));
    }
    q
}

/// `UID n:*` always yields the last message even when its UID is below `n`; drop those.
pub(crate) fn filter_new_uids(found: HashSet<u32>, last_uid: u32) -> Vec<u32> {
    let mut v: Vec<u32> = found.into_iter().filter(|u| *u > last_uid).collect();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_folder_name_handles_modified_utf7() {
        assert_eq!(decode_folder_name("Caixa de &AOcA4w-o"), "Caixa de ção");
        assert_eq!(decode_folder_name("INBOX"), "INBOX");
        // NB: the task brief writes "Caixa de &AOc-ao" for "ção"; that string is "çao" (U+00E7 + "ao").
        assert_eq!(decode_folder_name("Caixa de &AOc-ao"), "Caixa de çao");
        assert_eq!(decode_folder_name("A &- B"), "A & B");
        assert_eq!(decode_folder_name("&2D3eAA-"), "\u{1F600}");
    }

    #[test]
    fn decode_invalid_returns_input() {
        assert_eq!(decode_folder_name("bad &!!!- name"), "bad &!!!- name");
        assert_eq!(decode_folder_name("unterminated &AOc"), "unterminated &AOc");
    }

    #[test]
    fn encode_folder_name_matches_rfc_examples() {
        assert_eq!(encode_folder_name("Caixa de ção"), "Caixa de &AOcA4w-o");
        assert_eq!(encode_folder_name("INBOX"), "INBOX");
        assert_eq!(encode_folder_name("A & B"), "A &- B");
        // RFC 3501 example
        assert_eq!(
            encode_folder_name("~peter/mail/\u{53f0}\u{5317}/\u{65e5}\u{672c}\u{8a9e}"),
            "~peter/mail/&U,BTFw-/&ZeVnLIqe-"
        );
        // non-BMP goes through a surrogate pair
        assert_eq!(encode_folder_name("\u{1F600}"), "&2D3eAA-");
    }

    #[test]
    fn folder_name_roundtrips() {
        for n in [
            "INBOX",
            "Caixa de ção",
            "Enviados/Ação & Reação",
            "a&b&&c",
            "\u{1F600} smile \u{1F4E7}",
            "日本語.mail",
            "&",
            "ç",
            "Lixo.Eletrônico.2024",
        ] {
            assert_eq!(
                decode_folder_name(&encode_folder_name(n)),
                n,
                "roundtrip {n}"
            );
        }
    }

    #[test]
    fn uid_set_compresses_ranges() {
        assert_eq!(format_uid_set(&[1, 2, 3, 7]), "1:3,7");
        assert_eq!(format_uid_set(&[7, 1, 3, 2, 2]), "1:3,7");
        assert_eq!(format_uid_set(&[5]), "5");
        assert_eq!(format_uid_set(&[1, 3, 5]), "1,3,5");
        assert_eq!(format_uid_set(&[4, 5]), "4:5");
        assert_eq!(
            format_uid_set(&[u32::MAX - 1, u32::MAX]),
            "4294967294:4294967295"
        );
        assert_eq!(format_uid_set(&[]), "");
    }

    #[test]
    fn chunking_splits_sorted_unique() {
        let c = chunk_uids(&[5, 1, 2, 3, 4, 4], 2);
        assert_eq!(c, vec![vec![1, 2], vec![3, 4], vec![5]]);
        assert!(chunk_uids(&[], 10).is_empty());
        let big: Vec<u32> = (1..=2500).map(|i| i * 2).collect();
        let c = chunk_uids(&big, 1000);
        assert_eq!(c.len(), 3);
        assert_eq!(c.iter().map(Vec::len).sum::<usize>(), 2500);
    }

    #[test]
    fn imap_date_formats_utc() {
        assert_eq!(imap_date(0), "01-Jan-1970");
        assert_eq!(imap_date(1_700_000_000), "14-Nov-2023");
        assert_eq!(imap_date(951_782_400), "29-Feb-2000");
        assert_eq!(imap_date(86_399), "01-Jan-1970");
        assert_eq!(imap_date(-1), "31-Dec-1969");
    }

    #[test]
    fn search_query_shape() {
        assert_eq!(search_query(10, None), "UID 11:*");
        assert_eq!(
            search_query(0, Some(1_700_000_000)),
            "UID 1:* SINCE 14-Nov-2023"
        );
    }

    #[test]
    fn filter_drops_uids_at_or_below_last_uid() {
        let found: HashSet<u32> = [4, 9, 12, 10].into_iter().collect();
        assert_eq!(filter_new_uids(found, 9), vec![10, 12]);
        let only_last: HashSet<u32> = [4].into_iter().collect();
        assert!(filter_new_uids(only_last, 9).is_empty());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::auth::PasswordAuth;
    use crate::imap_conn::test_support::*;

    fn source(server: &Arc<Server>) -> ImapSource {
        let (sleep, _) = no_sleep();
        ImapSource::with_connector(
            Box::new(MockConnector(server.clone())),
            Box::new(PasswordAuth::new("alice", "hunter2")),
            RetryPolicy::default(),
            sleep,
        )
        .unwrap()
    }

    fn base(cmd: &str) -> Option<Reply> {
        if cmd.starts_with("LOGIN") {
            Some(Reply::Text("{tag} OK logged in\r\n".into()))
        } else {
            None
        }
    }

    fn fetch_line(uid: u32, flags: &str, body: &str) -> String {
        format!(
            "* {uid} FETCH (UID {uid} FLAGS ({flags}) INTERNALDATE \"14-Nov-2023 22:13:20 +0000\" BODY[] {{{}}}\r\n{body})\r\n",
            body.len()
        )
    }

    #[test]
    fn list_folders_decodes_skips_noselect_and_keeps_delimiter() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                Reply::Text(
                    "* LIST (\\HasChildren) \"/\" \"INBOX\"\r\n\
                     * LIST (\\Noselect \\HasChildren) \"/\" \"[Gmail]\"\r\n\
                     * LIST () \"/\" \"Caixa de &AOcA4w-o\"\r\n\
                     * LIST () \"/\" \"Pai/Filho &- Neto\"\r\n\
                     {tag} OK done\r\n"
                        .into(),
                )
            })
        });
        let mut s = source(&server);
        let f = s.list_folders().unwrap();
        assert_eq!(f, vec!["INBOX", "Caixa de ção", "Pai/Filho & Neto"]);
        assert!(server.commands().iter().any(|(_, c)| c == "LIST \"\" *"));
    }

    #[test]
    fn select_sends_modified_utf7_and_returns_state() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                if cmd.starts_with("LIST") {
                    // A server name that is not valid modified UTF-7 must round-trip untouched.
                    Reply::Text("* LIST () \".\" \"weird &!!- name\"\r\n{tag} OK\r\n".into())
                } else {
                    Reply::Text(
                        "* 2 EXISTS\r\n* OK [UIDVALIDITY 99] v\r\n* OK [UIDNEXT 17] n\r\n{tag} OK [READ-ONLY] ok\r\n"
                            .into(),
                    )
                }
            })
        });
        let mut s = source(&server);
        let st = s.select("Caixa de ção").unwrap();
        assert_eq!(
            st,
            FolderState {
                uidvalidity: 99,
                uidnext: 17
            }
        );
        s.list_folders().unwrap();
        s.select("weird &!!- name").unwrap();
        let cmds: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
        assert!(
            cmds.contains(&"EXAMINE \"Caixa de &AOcA4w-o\"".to_string()),
            "{cmds:?}"
        );
        assert!(
            cmds.contains(&"EXAMINE \"weird &!!- name\"".to_string()),
            "{cmds:?}"
        );
    }

    #[test]
    fn select_does_not_require_uidnext() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                Reply::Text(
                    "* 2 EXISTS\r\n* OK [UIDVALIDITY 42] v\r\n{tag} OK [READ-ONLY] ok\r\n".into(),
                )
            })
        });
        let mut s = source(&server);
        let st = s.select("INBOX").unwrap();
        assert_eq!(st.uidvalidity, 42);
        assert_eq!(st.uidnext, 0);
    }

    #[test]
    fn a_reconnect_while_opening_a_folder_never_reopens_the_previous_one() {
        // Conn 1: A opens, "Gone" is refused, the connection drops while opening B.
        // Conn 2: A no longer exists; B opens. The stale A must not be re-examined.
        let server = Server::new(|conn, cmd| {
            base(cmd).unwrap_or_else(|| {
                let ok = |uv: u32| {
                    Reply::Text(format!(
                        "* OK [UIDVALIDITY {uv}] v\r\n{{tag}} OK [READ-ONLY] ok\r\n"
                    ))
                };
                let no = || Reply::Text("{tag} NO no such mailbox\r\n".into());
                match (conn, cmd) {
                    (1, "EXAMINE \"A\"") => ok(1),
                    (1, "EXAMINE \"B\"") => Reply::Drop(String::new()),
                    (_, "EXAMINE \"B\"") => ok(2),
                    _ => no(),
                }
            })
        });
        let mut s = source(&server);
        s.select("A").unwrap();
        assert!(matches!(s.select("Gone"), Err(Error::Imap(_))));
        let st = s.select("B").unwrap();
        assert_eq!(st.uidvalidity, 2);
        let cmds = server.commands();
        assert!(
            !cmds.iter().any(|(c, x)| *c > 1 && x == "EXAMINE \"A\""),
            "{cmds:?}"
        );
    }

    #[test]
    fn select_still_requires_uidvalidity() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                Reply::Text(
                    "* 2 EXISTS\r\n* OK [UIDNEXT 3] n\r\n{tag} OK [READ-ONLY] ok\r\n".into(),
                )
            })
        });
        let mut s = source(&server);
        let r = s.select("INBOX");
        assert!(matches!(r, Err(Error::Imap(_))), "{r:?}");
    }

    #[test]
    fn dot_delimiter_server_is_presented_with_canonical_slash_and_mapped_back() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                if cmd.starts_with("LIST") {
                    Reply::Text(
                        "* LIST () \".\" \"INBOX\"\r\n\
                         * LIST (\\Noselect) \".\" \"Arquivo\"\r\n\
                         * LIST () \".\" \"INBOX.Enviados\"\r\n\
                         * LIST () \".\" \"Arquivo.Caixa de &AOcA4w-o\"\r\n\
                         {tag} OK done\r\n"
                            .into(),
                    )
                } else {
                    Reply::Text(
                        "* OK [UIDVALIDITY 5] v\r\n* OK [UIDNEXT 2] n\r\n{tag} OK [READ-ONLY] ok\r\n"
                            .into(),
                    )
                }
            })
        });
        let mut s = source(&server);
        let f = s.list_folders().unwrap();
        assert_eq!(f, vec!["INBOX", "INBOX/Enviados", "Arquivo/Caixa de ção"]);
        s.select("INBOX/Enviados").unwrap();
        s.select("Arquivo/Caixa de ção").unwrap();
        // INBOX is case-insensitive and also a hierarchy root; names never listed use the
        // delimiter learned from the server.
        s.select("inbox").unwrap();
        s.select("Inbox/Novo").unwrap();
        let cmds: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
        for want in [
            "EXAMINE \"INBOX.Enviados\"",
            "EXAMINE \"Arquivo.Caixa de &AOcA4w-o\"",
            "EXAMINE \"INBOX\"",
            "EXAMINE \"INBOX.Novo\"",
        ] {
            assert!(cmds.contains(&want.to_string()), "{want} not in {cmds:?}");
        }
    }

    #[test]
    fn uids_after_filters_the_last_message_quirk_and_sorts() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                if cmd.contains("UID 11:*") {
                    // Server returns only the highest existing UID (4 < 11).
                    Reply::Text("* SEARCH 4\r\n{tag} OK\r\n".into())
                } else {
                    Reply::Text("* SEARCH 30 12 20\r\n{tag} OK\r\n".into())
                }
            })
        });
        let mut s = source(&server);
        assert_eq!(s.uids_after(10, None).unwrap(), Vec::<u32>::new());
        assert_eq!(
            s.uids_after(5, Some(1_700_000_000)).unwrap(),
            vec![12, 20, 30]
        );
        let cmds: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
        assert!(
            cmds.contains(&"UID SEARCH UID 11:*".to_string()),
            "{cmds:?}"
        );
        assert!(
            cmds.contains(&"UID SEARCH UID 6:* SINCE 14-Nov-2023".to_string()),
            "{cmds:?}"
        );
    }

    #[test]
    fn fetch_returns_messages_with_flags_and_internal_date_and_uses_peek() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                let mut t = String::new();
                t.push_str(&fetch_line(
                    5,
                    "\\Seen \\Recent $Label",
                    "Subject: a\r\n\r\nhello",
                ));
                t.push_str(&fetch_line(7, "", "Subject: b\r\n\r\nworld"));
                t.push_str("{tag} OK done\r\n");
                Reply::Text(t)
            })
        });
        let mut s = source(&server);
        let m = s.fetch(&[7, 5, 6]).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].uid, 5);
        assert_eq!(m[0].flags, vec!["\\Seen".to_string(), "$Label".to_string()]);
        assert_eq!(m[0].internal_date, Some(1_700_000_000));
        assert_eq!(m[0].bytes, b"Subject: a\r\n\r\nhello".to_vec());
        assert_eq!(m[1].uid, 7);
        let cmds: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
        assert!(
            cmds.contains(&"UID FETCH 5:7 (UID FLAGS INTERNALDATE BODY.PEEK[])".to_string()),
            "{cmds:?}"
        );
    }

    #[test]
    fn fetch_errors_on_tagged_no() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                Reply::Text(format!(
                    "{}{{tag}} NO [SERVERBUG] fetch failed\r\n",
                    fetch_line(5, "", "partial")
                ))
            })
        });
        let mut s = source(&server);
        let r = s.fetch(&[5, 6]);
        assert!(matches!(r, Err(Error::Imap(_))), "{r:?}");
    }

    #[test]
    fn fetch_errors_when_connection_drops_mid_response_never_truncated_ok() {
        let server = Server::new(|conn, cmd| {
            base(cmd).unwrap_or_else(|| {
                if conn == 1 {
                    // First message complete, second cut off in the middle of its literal.
                    let mut t = fetch_line(5, "", "complete");
                    t.push_str("* 6 FETCH (UID 6 FLAGS () BODY[] {100}\r\nonly a bit");
                    Reply::Drop(t)
                } else {
                    Reply::Drop(String::new())
                }
            })
        });
        let mut s = source(&server);
        let r = s.fetch(&[5, 6]);
        assert!(matches!(r, Err(Error::Connection(_))), "{r:?}");
    }

    #[test]
    fn fetch_recovers_after_reconnect_and_reselects_folder() {
        let server = Server::new(|conn, cmd| {
            base(cmd).unwrap_or_else(|| {
                if cmd.starts_with("EXAMINE") {
                    Reply::Text(
                        "* OK [UIDVALIDITY 1] v\r\n* OK [UIDNEXT 9] n\r\n{tag} OK ok\r\n".into(),
                    )
                } else if conn == 1 {
                    Reply::Drop("* 5 FETCH (UID 5 BODY[] {50}\r\ntrunc".into())
                } else {
                    Reply::Text(format!("{}{{tag}} OK\r\n", fetch_line(5, "\\Seen", "full")))
                }
            })
        });
        let mut s = source(&server);
        s.select("INBOX").unwrap();
        let m = s.fetch(&[5]).unwrap();
        assert_eq!(m[0].bytes, b"full".to_vec());
        let on_conn2: Vec<String> = server
            .commands()
            .into_iter()
            .filter(|(c, _)| *c == 2)
            .map(|(_, c)| c)
            .collect();
        assert!(on_conn2[1].starts_with("EXAMINE"), "{on_conn2:?}");
        assert!(on_conn2[2].starts_with("UID FETCH"), "{on_conn2:?}");
    }

    #[test]
    fn message_ids_parse_header_fields_response() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                let h1 = "Message-ID: <abc@host>\r\n\r\n";
                let h2 = "\r\n"; // message without Message-ID
                Reply::Text(format!(
                    "* 1 FETCH (UID 5 BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{h1})\r\n\
                     * 2 FETCH (UID 6 BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{h2})\r\n\
                     {{tag}} OK\r\n",
                    h1.len(),
                    h2.len()
                ))
            })
        });
        let mut s = source(&server);
        let ids = s.message_ids(&[5, 6]).unwrap();
        assert_eq!(ids, vec![(5, Some("abc@host".to_string())), (6, None)]);
        let cmds: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
        assert!(
            cmds.contains(&"UID FETCH 5:6 (UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])".to_string()),
            "{cmds:?}"
        );
    }

    #[test]
    fn flags_returns_flag_strings() {
        let server = Server::new(|_, cmd| {
            base(cmd).unwrap_or_else(|| {
                Reply::Text(
                    "* 1 FETCH (UID 5 FLAGS (\\Seen \\Flagged))\r\n* 2 FETCH (UID 9 FLAGS ())\r\n{tag} OK\r\n"
                        .into(),
                )
            })
        });
        let mut s = source(&server);
        let f = s.flags(&[5, 9]).unwrap();
        assert_eq!(
            f,
            vec![
                (5, vec!["\\Seen".to_string(), "\\Flagged".to_string()]),
                (9, vec![])
            ]
        );
    }

    #[test]
    fn rejected_login_is_auth_error_after_one_attempt() {
        let server =
            Server::new(|_, _| Reply::Text("{tag} NO [AUTHENTICATIONFAILED] nope\r\n".into()));
        let (sleep, slept) = no_sleep();
        let err = ImapSource::with_connector(
            Box::new(MockConnector(server.clone())),
            Box::new(PasswordAuth::new("alice", "hunter2")),
            RetryPolicy::default(),
            sleep,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
        assert_eq!(server.connection_count(), 1);
        assert!(slept.lock().unwrap().is_empty());
    }

    #[test]
    fn parse_message_id_handles_folding_and_case() {
        assert_eq!(
            parse_message_id(b"message-id:\r\n <x@y>\r\n\r\n"),
            Some("x@y".into())
        );
        assert_eq!(
            parse_message_id(b"Subject: s\r\nMESSAGE-ID: <a@b>\r\n"),
            Some("a@b".into())
        );
        assert_eq!(parse_message_id(b"Subject: s\r\n"), None);
        assert_eq!(parse_message_id(b"Message-ID: <>\r\n"), None);
    }
}
