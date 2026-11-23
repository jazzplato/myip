//! Advice for a failure whose cause the user can fix. It is printed to stderr after the
//! error, once, and is never part of a document.

/// The text of the transport error when the platform verifier finds no CA certificates, as
/// seen in a container without `ca-certificates`. A library message, so a test pins a real run.
const NO_CA_CERTIFICATES: &str = "No CA certificates were loaded from the system";

pub const NO_CA_HINT: &str = "hint: no CA certificates were found; install ca-certificates, \
    or set SSL_CERT_FILE or SSL_CERT_DIR to a bundle";

/// Whether this transport error says there are no CA certificates to verify a server with.
pub fn is_no_ca(transport_error: &str) -> bool {
    transport_error.contains(NO_CA_CERTIFICATES)
}
