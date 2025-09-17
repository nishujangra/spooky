use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::fs;

pub fn load_tls(
    cert_path: &str,
    key_path: &str,
) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let cert_bytes = fs::read(cert_path).expect("Failed to read cert file");
    let key_bytes = fs::read(key_path).expect("Failed to read key file");

    let certs = vec![CertificateDer::from(cert_bytes)];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_bytes));

    (certs, key)
}
