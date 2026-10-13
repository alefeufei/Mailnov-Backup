//! Profiles: non-secret configuration stored as `<config>/profiles/<name>.toml`.

use crate::tls::TlsMode;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub user: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetSpec {
    Imap(Endpoint),
    EmlDir(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub archive_dir: PathBuf,
    pub source: Endpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<String>>,
    /// `YYYY-MM-DD`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetSpec>,
}

impl Profile {
    /// UTC midnight (unix seconds) of the `since` date, if set. Errors on an invalid date.
    pub fn since_epoch(&self) -> Result<Option<i64>> {
        let Some(s) = &self.since else {
            return Ok(None);
        };
        let bad = || Error::Config(format!("invalid 'since' date '{s}': expected YYYY-MM-DD"));
        let b = s.as_bytes();
        let shape_ok = b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
        if !shape_ok {
            return Err(bad());
        }
        let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?;
        let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        Ok(Some(midnight.and_utc().timestamp()))
    }
}

/// Validates a profile name (used in file names and credential-store keys).
pub fn validate_name(name: &str) -> Result<()> {
    let bad = |why: &str| Error::Config(format!("invalid profile name '{name}': {why}"));
    if name.is_empty() {
        return Err(bad("must not be empty"));
    }
    if name.len() > 64 {
        return Err(bad("at most 64 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(bad("only letters, digits, '.', '_' and '-' are allowed"));
    }
    if name.starts_with('.') || name.ends_with('.') {
        return Err(bad("must not start or end with '.'"));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err(bad("reserved name"));
    }
    Ok(())
}

pub struct ProfileStore {
    dir: PathBuf,
}

impl ProfileStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }
    fn profiles_dir(&self) -> PathBuf {
        self.dir.join("profiles")
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.profiles_dir().join(format!("{name}.toml")))
    }

    /// An existing profile whose name equals `name` ignoring ASCII case but is spelled
    /// differently. Profile files live on case-insensitive file systems (Windows), so such
    /// names would share one file.
    pub fn case_conflict(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .list()?
            .into_iter()
            .find(|n| n != name && n.eq_ignore_ascii_case(name)))
    }

    /// Writes atomically (temp file in the same directory, then rename). Refuses a name that
    /// differs only by case from an existing profile.
    pub fn save(&self, p: &Profile) -> Result<()> {
        let path = self.path_for(&p.name)?;
        if let Some(other) = self.case_conflict(&p.name)? {
            return Err(Error::Config(format!(
                "profile '{}' conflicts with the existing profile '{other}' \
                 (profile names are case-insensitive)",
                p.name
            )));
        }
        let text = toml::to_string(p)
            .map_err(|e| Error::Config(format!("cannot serialize profile '{}': {e}", p.name)))?;
        std::fs::create_dir_all(self.profiles_dir())?;
        let tmp = self.profiles_dir().join(format!("{}.toml.tmp", p.name));
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::Io(e)
        })
    }

    pub fn load(&self, name: &str) -> Result<Profile> {
        let path = self.path_for(name)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Config(format!("profile '{name}' does not exist")))
            }
            Err(e) => return Err(Error::Io(e)),
        };
        let p: Profile = toml::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "profile '{name}' is invalid: {}. Profiles hold no secrets; passwords belong \
                 in the credential store (use `profile add`/`profile set-password`)",
                e.message()
            ))
        })?;
        if p.name != name && p.name.eq_ignore_ascii_case(name) {
            return Err(Error::Config(format!(
                "profile '{name}' does not exist; did you mean '{}'? \
                 (profile names are case-insensitive)",
                p.name
            )));
        }
        if p.name != name {
            return Err(Error::Config(format!(
                "profile file '{name}.toml' declares a different name '{}'",
                p.name
            )));
        }
        Ok(p)
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let rd = match std::fs::read_dir(self.profiles_dir()) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut names = Vec::new();
        for entry in rd {
            let file = entry?.file_name();
            let Some(file) = file.to_str() else { continue };
            if let Some(stem) = file.strip_suffix(".toml") {
                if validate_name(stem).is_ok() {
                    names.push(stem.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.path_for(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::Config(format!("profile '{name}' does not exist")))
            }
            Err(e) => Err(Error::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(host: &str) -> Endpoint {
        Endpoint {
            host: host.into(),
            port: 993,
            tls: TlsMode::Implicit,
            user: "u@x.com".into(),
        }
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            archive_dir: PathBuf::from("/data/archive"),
            source: ep("imap.x.com"),
            folders: Some(vec!["INBOX".into(), "Sent".into()]),
            since: Some("2024-01-31".into()),
            target: Some(TargetSpec::Imap(Endpoint {
                host: "h".into(),
                port: 143,
                tls: TlsMode::StartTls,
                user: "t".into(),
            })),
        }
    }

    fn store() -> (tempfile::TempDir, ProfileStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ProfileStore::new(d.path());
        (d, s)
    }

    #[test]
    fn profile_roundtrips_through_toml() {
        let (_d, s) = store();
        for target in [
            None,
            Some(TargetSpec::EmlDir(PathBuf::from("/out"))),
            profile("a").target,
        ] {
            let mut p = profile("a");
            p.target = target;
            s.save(&p).unwrap();
            assert_eq!(s.load("a").unwrap(), p);
        }
        let mut min = profile("a");
        min.folders = None;
        min.since = None;
        s.save(&min).unwrap();
        assert_eq!(s.load("a").unwrap(), min);
    }

    #[test]
    fn toml_shape_uses_lowercase_tls() {
        let text = toml::to_string(&profile("a")).unwrap();
        assert!(text.contains("tls = \"implicit\""), "{text}");
        assert!(text.contains("tls = \"starttls\""), "{text}");
    }

    #[test]
    fn profile_toml_never_contains_password() {
        let (d, s) = store();
        s.save(&profile("a")).unwrap();
        let text = std::fs::read_to_string(d.path().join("profiles/a.toml")).unwrap();
        assert!(!text.to_lowercase().contains("password"), "{text}");
        assert!(!text.to_lowercase().contains("secret"), "{text}");
    }

    #[test]
    fn hand_added_password_field_is_rejected() {
        let (d, s) = store();
        s.save(&profile("a")).unwrap();
        let path = d.path().join("profiles/a.toml");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replacen("[source]", "[source]\npassword = \"hunter2\"", 1);
        std::fs::write(&path, text).unwrap();
        let err = s.load("a").unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        let msg = err.to_string();
        assert!(msg.contains("credential store"), "{msg}");
        assert!(!msg.contains("hunter2"), "{msg}");
    }

    #[test]
    fn load_unknown_profile_is_config_error() {
        let (_d, s) = store();
        let err = s.load("nope").unwrap_err();
        assert!(
            matches!(err, Error::Config(ref m) if m.contains("nope")),
            "{err}"
        );
    }

    #[test]
    fn remove_unknown_is_config_error_and_known_is_removed() {
        let (_d, s) = store();
        assert!(matches!(s.remove("nope"), Err(Error::Config(_))));
        s.save(&profile("a")).unwrap();
        s.remove("a").unwrap();
        assert!(matches!(s.load("a"), Err(Error::Config(_))));
    }

    #[test]
    fn list_is_sorted_and_empty_dir_is_ok() {
        let (_d, s) = store();
        assert!(s.list().unwrap().is_empty());
        for n in ["zeta", "alpha", "Mid"] {
            s.save(&profile(n)).unwrap();
        }
        let mut expected = vec!["Mid".to_string(), "alpha".into(), "zeta".into()];
        expected.sort();
        assert_eq!(s.list().unwrap(), expected);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp() {
        let (d, s) = store();
        s.save(&profile("a")).unwrap();
        s.save(&profile("a")).unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path().join("profiles"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.toml".to_string()]);
    }

    #[test]
    fn name_validation() {
        let max = "A".repeat(64);
        for ok in ["a", "work", "my-box_1.2", max.as_str()] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in [
            "",
            ".hidden",
            "..",
            "../../x",
            "a/b",
            "a\\b",
            "a b",
            "a:b",
            "ç",
            "a.",
            "CON",
            "con",
            "nul.txt",
            "COM1",
            "lpt9",
            "Aux",
            "a\0b",
            long.as_str(),
        ] {
            assert!(
                matches!(validate_name(bad), Err(Error::Config(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn save_load_remove_reject_bad_names() {
        let (d, s) = store();
        let mut p = profile("../../x");
        assert!(matches!(s.save(&p), Err(Error::Config(_))));
        p.name = "ok".into();
        s.save(&p).unwrap();
        assert!(matches!(s.load("../ok"), Err(Error::Config(_))));
        assert!(matches!(s.remove("../ok"), Err(Error::Config(_))));
        assert!(!d.path().join("x.toml").exists());
    }

    #[test]
    fn load_rejects_name_mismatch() {
        let (d, s) = store();
        s.save(&profile("a")).unwrap();
        std::fs::copy(
            d.path().join("profiles/a.toml"),
            d.path().join("profiles/b.toml"),
        )
        .unwrap();
        assert!(matches!(s.load("b"), Err(Error::Config(_))));
    }

    #[test]
    fn names_differing_only_by_case_never_overwrite_each_other() {
        let (_d, s) = store();
        s.save(&profile("work")).unwrap();
        let err = s.save(&profile("Work")).unwrap_err();
        assert!(
            matches!(err, Error::Config(ref m) if m.contains("'work'")),
            "{err}"
        );
        assert_eq!(s.load("work").unwrap(), profile("work"));
        assert_eq!(s.list().unwrap(), vec!["work".to_string()]);
        assert_eq!(s.case_conflict("WORK").unwrap(), Some("work".to_string()));
        assert_eq!(s.case_conflict("work").unwrap(), None);
        // the right spelling is suggested when looking a profile up with the wrong case
        if let Err(Error::Config(m)) = s.load("WORK") {
            assert!(m.contains("'work'"), "{m}");
        }
    }

    #[test]
    fn since_must_be_valid_date() {
        for bad in [
            "2024-13-40",
            "2024-02-30",
            "2023-02-29",
            "2024-1-5",
            "20240105",
            "",
            "abc",
            "2024-01-05 ",
            "+024-01-05",
            "2024-00-10",
            "2024-01-00",
        ] {
            let mut p = profile("a");
            p.since = Some(bad.into());
            assert!(matches!(p.since_epoch(), Err(Error::Config(_))), "{bad:?}");
        }
    }

    #[test]
    fn since_epoch_is_utc_midnight() {
        let mut p = profile("a");
        p.since = Some("1970-01-02".into());
        assert_eq!(p.since_epoch().unwrap(), Some(86_400));
        p.since = Some("2024-02-29".into());
        assert_eq!(p.since_epoch().unwrap(), Some(1_709_164_800));
        p.since = None;
        assert_eq!(p.since_epoch().unwrap(), None);
    }
}
