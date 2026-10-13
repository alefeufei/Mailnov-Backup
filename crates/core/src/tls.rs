/// How the connection to the server is secured. There is deliberately no plaintext variant and no
/// way to disable certificate validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// TLS from the first byte (usually port 993).
    Implicit,
    /// Plaintext greeting, then mandatory upgrade with STARTTLS (usually port 143). If the server
    /// does not offer STARTTLS the connection fails; it never continues unencrypted.
    StartTls,
}
