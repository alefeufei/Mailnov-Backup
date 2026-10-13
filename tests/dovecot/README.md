# Dovecot integration fixture (work in progress, finished in Task 8)

`crates/core/tests/imap_dovecot.rs::archive_from_dovecot` is `#[ignore]`d and needs a Dovecot with:

- IMAPS on port 993, user `testuser` / password `testpass` (override with `MAILBACKUP_TEST_IMAP_HOST|PORT|USER|PASSWORD`);
- the messages in `seed/*.eml` delivered to `INBOX` unmodified;
- a certificate for `localhost` that the client **trusts**. `mailbackup` has no option to skip
  certificate validation, so create a local CA, sign the server certificate with it and add the CA
  to the OS trust store (rustls reads the OS roots; on Linux/macOS `SSL_CERT_FILE` also works).

The compose file here is only a skeleton: the Dovecot `conf/` and `certs/` directories are not
written yet and it has never been run.
