//! Best-effort IMAP server guess from an e-mail address.

use crate::profile::Endpoint;
use crate::tls::TlsMode;

/// Domains whose IMAP host is not simply `imap.<domain>`.
const KNOWN_HOSTS: &[(&str, &str)] = &[
    ("gmail.com", "imap.gmail.com"),
    ("outlook.com", "outlook.office365.com"),
    ("hotmail.com", "outlook.office365.com"),
    ("live.com", "outlook.office365.com"),
    ("yahoo.com", "imap.mail.yahoo.com"),
    ("icloud.com", "imap.mail.me.com"),
];

/// Guesses the IMAP endpoint for `email`: `imap.<domain>:993`, implicit TLS, user = full address.
/// Returns `None` for malformed addresses.
pub fn guess_imap(email: &str) -> Option<Endpoint> {
    if email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || domain.contains('@') {
        return None;
    }
    let domain = domain.to_ascii_lowercase();
    // Need at least two non-empty labels ("x.com").
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    let host = KNOWN_HOSTS
        .iter()
        .find(|(d, _)| *d == domain)
        .map(|(_, h)| (*h).to_string())
        .unwrap_or_else(|| format!("imap.{domain}"));
    Some(Endpoint {
        host,
        port: 993,
        tls: TlsMode::Implicit,
        user: email.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_imap_uses_domain_and_port_993() {
        let e = guess_imap("a@exemplo.com.br").unwrap();
        assert_eq!(e.host, "imap.exemplo.com.br");
        assert_eq!(e.port, 993);
        assert_eq!(e.tls, TlsMode::Implicit);
        assert_eq!(e.user, "a@exemplo.com.br");
    }

    #[test]
    fn guess_imap_lowercases_domain_keeps_user() {
        let e = guess_imap("Foo@Exemplo.COM").unwrap();
        assert_eq!(e.host, "imap.exemplo.com");
        assert_eq!(e.user, "Foo@Exemplo.COM");
    }

    #[test]
    fn guess_imap_known_providers() {
        for (email, host) in [
            ("x@gmail.com", "imap.gmail.com"),
            ("x@Outlook.com", "outlook.office365.com"),
            ("x@hotmail.com", "outlook.office365.com"),
            ("x@live.com", "outlook.office365.com"),
            ("x@yahoo.com", "imap.mail.yahoo.com"),
            ("x@icloud.com", "imap.mail.me.com"),
        ] {
            let e = guess_imap(email).unwrap();
            assert_eq!(e.host, host, "{email}");
            assert_eq!(e.port, 993);
        }
    }

    #[test]
    fn guess_imap_rejects_malformed() {
        for bad in [
            "",
            "plain",
            "@x.com",
            "a@",
            "a@@x.com",
            "a b@x.com",
            "a@x .com",
            " a@x.com",
            "a@x.com ",
            "a@.com",
            "a@x.",
            "a@x..com",
            "a@nodot",
            "a\t@x.com",
        ] {
            assert!(guess_imap(bad).is_none(), "{bad:?}");
        }
    }
}
