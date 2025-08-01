// This is a test shim for the BoringSSL-Go ('bogo') TLS
// test suite. See bogo/ for this in action.
//
// https://boringssl.googlesource.com/boringssl/+/master/ssl/test
//

#![warn(
    clippy::alloc_instead_of_core,
    clippy::manual_let_else,
    clippy::std_instead_of_core,
    clippy::use_self,
    clippy::upper_case_acronyms,
    elided_lifetimes_in_paths,
    trivial_casts,
    trivial_numeric_casts,
    unreachable_pub,
    unused_import_braces,
    unused_extern_crates,
    unused_qualifications
)]

use core::fmt::{Debug, Formatter};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::{env, net, process, thread, time};

use base64::prelude::{BASE64_STANDARD, Engine};
#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{
    ClientConfig, ClientConnection, EchConfig, EchGreaseConfig, EchMode, EchStatus, Resumption,
    Tls12Resumption, WebPkiServerVerifier,
};
use rustls::crypto::aws_lc_rs::hpke;
use rustls::crypto::hpke::{Hpke, HpkePublicKey};
use rustls::crypto::{CryptoProvider, aws_lc_rs, ring};
use rustls::internal::msgs::codec::Codec;
use rustls::internal::msgs::persist::ServerSessionValue;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{
    CertificateDer, EchConfigListBytes, PrivateKeyDer, ServerName, SubjectPublicKeyInfoDer,
    UnixTime,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{
    ClientHello, ProducesTickets, ServerConfig, ServerConnection, WebPkiClientVerifier,
};
use rustls::{
    AlertDescription, CertificateCompressionAlgorithm, CertificateError, Connection,
    DigitallySignedStruct, DistinguishedName, Error, HandshakeKind, InvalidMessage, NamedGroup,
    PeerIncompatible, PeerMisbehaved, ProtocolVersion, RootCertStore, Side, SignatureAlgorithm,
    SignatureScheme, SupportedProtocolVersion, client, compress, server, sign, version,
};

static BOGO_NACK: i32 = 89;

macro_rules! println_err(
  ($($arg:tt)*) => { {
    writeln!(&mut ::std::io::stderr(), $($arg)*).unwrap();
  } }
);

#[derive(Debug)]
struct Options {
    port: u16,
    shim_id: u64,
    side: Side,
    max_fragment: Option<usize>,
    resumes: usize,
    verify_peer: bool,
    require_any_client_cert: bool,
    server_preference: bool,
    root_hint_subjects: Vec<DistinguishedName>,
    offer_no_client_cas: bool,
    tickets: bool,
    resume_with_tickets_disabled: bool,
    queue_data: bool,
    queue_data_on_resume: bool,
    only_write_one_byte_after_handshake: bool,
    only_write_one_byte_after_handshake_on_resume: bool,
    shut_down_after_handshake: bool,
    check_close_notify: bool,
    host_name: String,
    use_sni: bool,
    trusted_cert_file: String,
    credentials: Credentials,
    protocols: Vec<String>,
    reject_alpn: bool,
    support_tls13: bool,
    support_tls12: bool,
    min_version: Option<ProtocolVersion>,
    max_version: Option<ProtocolVersion>,
    server_ocsp_response: Vec<u8>,
    groups: Option<Vec<NamedGroup>>,
    export_keying_material: usize,
    export_keying_material_label: String,
    export_keying_material_context: String,
    export_keying_material_context_used: bool,
    export_traffic_secrets: bool,
    read_size: usize,
    quic_transport_params: Vec<u8>,
    expect_quic_transport_params: Vec<u8>,
    enable_early_data: bool,
    expect_ticket_supports_early_data: bool,
    expect_accept_early_data: bool,
    expect_reject_early_data: bool,
    expect_version: u16,
    resumption_delay: u32,
    queue_early_data_after_received_messages: Vec<usize>,
    require_ems: bool,
    expect_handshake_kind: Option<Vec<HandshakeKind>>,
    expect_handshake_kind_resumed: Option<Vec<HandshakeKind>>,
    install_cert_compression_algs: CompressionAlgs,
    selected_provider: SelectedProvider,
    provider: CryptoProvider,
    ech_config_list: Option<EchConfigListBytes<'static>>,
    expect_ech_accept: bool,
    expect_ech_retry_configs: Option<EchConfigListBytes<'static>>,
    on_resume_ech_config_list: Option<EchConfigListBytes<'static>>,
    on_resume_expect_ech_accept: bool,
    on_initial_expect_ech_accept: bool,
    enable_ech_grease: bool,
    send_key_update: bool,
    expect_curve_id: Option<NamedGroup>,
    on_initial_expect_curve_id: Option<NamedGroup>,
    on_resume_expect_curve_id: Option<NamedGroup>,
    wait_for_debugger: bool,
    ocsp: OcspValidation,
}

impl Options {
    fn new() -> Self {
        let selected_provider = SelectedProvider::from_env();
        Self {
            port: 0,
            shim_id: 0,
            side: Side::Client,
            max_fragment: None,
            resumes: 0,
            verify_peer: false,
            tickets: true,
            resume_with_tickets_disabled: false,
            host_name: "example.com".to_string(),
            use_sni: false,
            queue_data: false,
            queue_data_on_resume: false,
            only_write_one_byte_after_handshake: false,
            only_write_one_byte_after_handshake_on_resume: false,
            shut_down_after_handshake: false,
            check_close_notify: false,
            require_any_client_cert: false,
            server_preference: false,
            root_hint_subjects: vec![],
            offer_no_client_cas: false,
            trusted_cert_file: "".to_string(),
            credentials: Credentials::default(),
            protocols: vec![],
            reject_alpn: false,
            support_tls13: true,
            support_tls12: true,
            min_version: None,
            max_version: None,
            server_ocsp_response: vec![],
            groups: None,
            export_keying_material: 0,
            export_keying_material_label: "".to_string(),
            export_keying_material_context: "".to_string(),
            export_keying_material_context_used: false,
            export_traffic_secrets: false,
            read_size: 512,
            quic_transport_params: vec![],
            expect_quic_transport_params: vec![],
            enable_early_data: false,
            expect_ticket_supports_early_data: false,
            expect_accept_early_data: false,
            expect_reject_early_data: false,
            expect_version: 0,
            resumption_delay: 0,
            queue_early_data_after_received_messages: vec![],
            require_ems: false,
            expect_handshake_kind: None,
            expect_handshake_kind_resumed: Some(vec![HandshakeKind::Resumed]),
            install_cert_compression_algs: CompressionAlgs::None,
            selected_provider,
            provider: selected_provider.provider(),
            ech_config_list: None,
            expect_ech_accept: false,
            expect_ech_retry_configs: None,
            on_resume_ech_config_list: None,
            on_resume_expect_ech_accept: false,
            on_initial_expect_ech_accept: false,
            enable_ech_grease: false,
            send_key_update: false,
            expect_curve_id: None,
            on_initial_expect_curve_id: None,
            on_resume_expect_curve_id: None,
            wait_for_debugger: false,
            ocsp: OcspValidation::default(),
        }
    }

    fn version_allowed(&self, vers: ProtocolVersion) -> bool {
        (self.min_version.is_none() || u16::from(vers) >= u16::from(self.min_version.unwrap()))
            && (self.max_version.is_none()
                || u16::from(vers) <= u16::from(self.max_version.unwrap()))
    }

    fn tls13_supported(&self) -> bool {
        self.support_tls13 && self.version_allowed(ProtocolVersion::TLSv1_3)
    }

    fn tls12_supported(&self) -> bool {
        self.support_tls12 && self.version_allowed(ProtocolVersion::TLSv1_2)
    }

    fn supported_versions(&self) -> Vec<&'static SupportedProtocolVersion> {
        let mut versions = vec![];

        if self.tls12_supported() {
            versions.push(&version::TLS12);
        }

        if self.tls13_supported() {
            versions.push(&version::TLS13);
        }
        versions
    }
}

#[derive(Debug, Default)]
struct Credentials {
    default: Credential,
    additional: Vec<Credential>,
    /// Some(-1) means `default`, otherwise index into `additional`
    expect_selected: Option<isize>,
}

impl Credentials {
    fn last_mut(&mut self) -> &mut Credential {
        self.additional
            .last_mut()
            .unwrap_or(&mut self.default)
    }

    fn configured(&self) -> bool {
        self.default.configured()
            || self
                .additional
                .iter()
                .any(|cred| cred.configured())
    }
}

#[derive(Clone, Debug, Default)]
struct Credential {
    key_file: String,
    cert_file: String,
    use_signing_scheme: Option<u16>,
    must_match_issuer: bool,
}

impl Credential {
    fn load_from_file(&self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let certs = CertificateDer::pem_file_iter(&self.cert_file)
            .unwrap()
            .map(|cert| cert.unwrap())
            .collect::<Vec<_>>();
        let key = PrivateKeyDer::from_pem_file(&self.key_file).unwrap();
        (certs, key)
    }

    fn configured(&self) -> bool {
        !self.cert_file.is_empty() && !self.key_file.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SelectedProvider {
    AwsLcRs,
    #[cfg_attr(not(feature = "fips"), allow(dead_code))]
    AwsLcRsFips,
    Ring,
}

impl SelectedProvider {
    fn from_env() -> Self {
        match env::var("BOGO_SHIM_PROVIDER")
            .ok()
            .as_deref()
        {
            None | Some("aws-lc-rs") => Self::AwsLcRs,
            #[cfg(feature = "fips")]
            Some("aws-lc-rs-fips") => Self::AwsLcRsFips,
            Some("ring") => Self::Ring,
            Some(other) => panic!("unrecognised value for BOGO_SHIM_PROVIDER: {other:?}"),
        }
    }

    fn provider(&self) -> CryptoProvider {
        match self {
            Self::AwsLcRs | Self::AwsLcRsFips => {
                // ensure all suites and kx groups are included (even in fips builds)
                // as non-fips test cases require them.  runner activates fips mode via -fips-202205 option
                // this includes rustls-post-quantum, which just returns an altered
                // version of `aws_lc_rs::default_provider()`
                CryptoProvider {
                    kx_groups: aws_lc_rs::DEFAULT_KX_GROUPS.to_vec(),
                    cipher_suites: aws_lc_rs::ALL_CIPHER_SUITES.to_vec(),
                    ..aws_lc_rs::default_provider()
                }
            }

            Self::Ring => ring::default_provider(),
        }
    }

    fn ticketer(&self) -> Arc<dyn ProducesTickets> {
        match self {
            Self::AwsLcRs | Self::AwsLcRsFips => aws_lc_rs::Ticketer::new().unwrap(),
            Self::Ring => ring::Ticketer::new().unwrap(),
        }
    }

    fn supports_ech(&self) -> bool {
        match *self {
            Self::AwsLcRs | Self::AwsLcRsFips => true,
            Self::Ring => false,
        }
    }
}

fn load_root_certs(filename: &str) -> Arc<RootCertStore> {
    let mut roots = RootCertStore::empty();

    // -verify-peer can be used without specifying a root cert,
    // to test (eg) client auth without actually looking at the certs.
    //
    // but WebPkiClientVerifier requires a non-empty set of roots.
    //
    // use an unrelated cert we have lying around.
    let filename = match filename {
        "" => "../../../../../test-ca/rsa-2048/ca.cert",
        filename => filename,
    };

    roots.add_parsable_certificates(
        CertificateDer::pem_file_iter(filename)
            .unwrap()
            .map(|item| item.unwrap()),
    );
    Arc::new(roots)
}

fn split_protocols(protos: &str) -> Vec<String> {
    let mut ret = Vec::new();

    let mut offs = 0;
    while offs < protos.len() {
        let len = protos.as_bytes()[offs] as usize;
        let item = protos[offs + 1..offs + 1 + len].to_string();
        ret.push(item);
        offs += 1 + len;
    }

    ret
}

fn decode_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .inspect(|x| println!("item {x:?}"))
        .collect()
}

#[derive(Debug)]
struct DummyClientAuth {
    mandatory: bool,
    root_hint_subjects: Arc<[DistinguishedName]>,
    parent: Arc<dyn ClientCertVerifier>,
}

impl DummyClientAuth {
    fn new(
        trusted_cert_file: &str,
        mandatory: bool,
        root_hint_subjects: Arc<[DistinguishedName]>,
    ) -> Self {
        Self {
            mandatory,
            root_hint_subjects,
            parent: WebPkiClientVerifier::builder_with_provider(
                load_root_certs(trusted_cert_file),
                SelectedProvider::from_env()
                    .provider()
                    .into(),
            )
            .build()
            .unwrap(),
        }
    }
}

impl ClientCertVerifier for DummyClientAuth {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> Arc<[DistinguishedName]> {
        self.root_hint_subjects.clone()
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.parent
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.parent
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.parent.supported_verify_schemes()
    }
}

#[derive(Debug)]
struct DummyServerAuth {
    parent: Arc<dyn ServerCertVerifier>,
    ocsp: OcspValidation,
}

impl DummyServerAuth {
    fn new(trusted_cert_file: &str, ocsp: OcspValidation) -> Self {
        Self {
            parent: WebPkiServerVerifier::builder_with_provider(
                load_root_certs(trusted_cert_file),
                SelectedProvider::from_env()
                    .provider()
                    .into(),
            )
            .build()
            .unwrap(),
            ocsp,
        }
    }
}

impl ServerCertVerifier for DummyServerAuth {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _certs: &[CertificateDer<'_>],
        _hostname: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if let OcspValidation::Reject = self.ocsp {
            return Err(CertificateError::InvalidOcspResponse.into());
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.parent
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.parent
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.parent.supported_verify_schemes()
    }

    fn request_ocsp_response(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum OcspValidation {
    /// Totally ignore `ocsp_response` value
    #[default]
    None,

    /// Return an error (irrespective of `ocsp_response` value)
    Reject,
}

#[derive(Debug)]
struct FixedSignatureSchemeSigningKey {
    key: Arc<dyn sign::SigningKey>,
    scheme: SignatureScheme,
}

impl sign::SigningKey for FixedSignatureSchemeSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn sign::Signer>> {
        if offered.contains(&self.scheme) {
            self.key.choose_scheme(&[self.scheme])
        } else {
            self.key.choose_scheme(&[])
        }
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        self.key.public_key()
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        self.key.algorithm()
    }
}

#[derive(Debug)]
struct FixedSignatureSchemeServerCertResolver {
    resolver: Arc<dyn server::ResolvesServerCert>,
    scheme: SignatureScheme,
}

impl server::ResolvesServerCert for FixedSignatureSchemeServerCertResolver {
    fn resolve(&self, client_hello: &ClientHello<'_>) -> Option<Arc<sign::CertifiedKey>> {
        let mut certkey = self.resolver.resolve(client_hello)?;
        Arc::make_mut(&mut certkey).key = Arc::new(FixedSignatureSchemeSigningKey {
            key: certkey.key.clone(),
            scheme: self.scheme,
        });
        Some(certkey)
    }
}

#[derive(Debug, Default)]
struct MultipleClientCredentialResolver {
    additional: Vec<ClientCert>,
    default: Option<ClientCert>,
    expect_selected: Option<isize>,
}

impl MultipleClientCredentialResolver {
    fn add(&mut self, key: sign::CertifiedKey, meta: &Credential) {
        self.additional
            .push(ClientCert::new(key, meta));
    }

    fn set_default(&mut self, key: sign::CertifiedKey, meta: &Credential) {
        self.default = Some(ClientCert::new(key, meta));
    }
}

impl client::ResolvesClientCert for MultipleClientCredentialResolver {
    fn resolve(
        &self,
        root_hint_subjects: &[&[u8]],
        sig_schemes: &[SignatureScheme],
    ) -> Option<Arc<sign::CertifiedKey>> {
        // `sig_schemes` is in server preference order, so respect that.
        for sig_scheme in sig_schemes.iter().copied() {
            for (i, cert) in self.additional.iter().enumerate() {
                // if the server sends any issuer hints, respect them
                if cert.must_match_issuer
                    && !root_hint_subjects
                        .iter()
                        .any(|dn| *dn == cert.issuer_dn.as_ref())
                {
                    continue;
                }

                if cert
                    .certkey
                    .key
                    .choose_scheme(&[sig_scheme])
                    .is_some()
                {
                    assert!(
                        Some(i as isize) == self.expect_selected || self.expect_selected.is_none()
                    );
                    return Some(cert.certkey.clone());
                }
            }
        }

        if let Some(cert) = &self.default {
            if cert
                .certkey
                .key
                .choose_scheme(sig_schemes)
                .is_some()
            {
                assert!(matches!(self.expect_selected, Some(-1) | None));
                return Some(cert.certkey.clone());
            }
        }

        assert_eq!(self.expect_selected, None);

        let all_must_match_issuer = self
            .additional
            .iter()
            .chain(self.default.iter())
            .all(|item| item.must_match_issuer);

        quit(match all_must_match_issuer {
            true => ":NO_MATCHING_ISSUER:",
            false => ":NO_COMMON_SIGNATURE_ALGORITHMS:",
        })
    }

    fn has_certs(&self) -> bool {
        self.default.is_some() || !self.additional.is_empty()
    }
}

#[derive(Debug)]
struct ClientCert {
    certkey: Arc<sign::CertifiedKey>,
    issuer_dn: DistinguishedName,
    must_match_issuer: bool,
}

impl ClientCert {
    fn new(mut certkey: sign::CertifiedKey, meta: &Credential) -> Self {
        let parsed_cert =
            webpki::EndEntityCert::try_from(certkey.cert_chain.last().unwrap()).unwrap();
        let issuer_dn = DistinguishedName::in_sequence(parsed_cert.issuer());

        if let Some(scheme) = meta.use_signing_scheme {
            certkey.key = Arc::new(FixedSignatureSchemeSigningKey {
                key: certkey.key,
                scheme: lookup_scheme(scheme),
            });
        }

        Self {
            certkey: Arc::new(certkey),
            issuer_dn,
            must_match_issuer: meta.must_match_issuer,
        }
    }
}

fn lookup_scheme(scheme: u16) -> SignatureScheme {
    match scheme {
        0x0401 => SignatureScheme::RSA_PKCS1_SHA256,
        0x0501 => SignatureScheme::RSA_PKCS1_SHA384,
        0x0601 => SignatureScheme::RSA_PKCS1_SHA512,
        0x0403 => SignatureScheme::ECDSA_NISTP256_SHA256,
        0x0503 => SignatureScheme::ECDSA_NISTP384_SHA384,
        0x0603 => SignatureScheme::ECDSA_NISTP521_SHA512,
        0x0804 => SignatureScheme::RSA_PSS_SHA256,
        0x0805 => SignatureScheme::RSA_PSS_SHA384,
        0x0806 => SignatureScheme::RSA_PSS_SHA512,
        0x0807 => SignatureScheme::ED25519,
        // TODO: add support for Ed448
        // 0x0808 => SignatureScheme::ED448,
        _ => {
            println_err!("Unsupported signature scheme {:04x}", scheme);
            process::exit(BOGO_NACK);
        }
    }
}

#[derive(Debug)]
struct ServerCacheWithResumptionDelay {
    delay: u32,
    storage: Arc<dyn server::StoresServerSessions>,
}

impl ServerCacheWithResumptionDelay {
    fn new(delay: u32) -> Arc<Self> {
        Arc::new(Self {
            delay,
            storage: server::ServerSessionMemoryCache::new(32),
        })
    }
}

fn align_time() {
    /* we don't have an injectable clock source in rustls' public api, and
     * resumption timing is in seconds resolution, so tests that use
     * resumption_delay tend to be flickery if the seconds time ticks
     * during this.
     *
     * this function delays until a fresh second ticks, which alleviates
     * this. gross!
     */
    fn sample() -> u64 {
        time::SystemTime::now()
            .duration_since(time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    let start_secs = sample();
    while start_secs == sample() {
        thread::sleep(time::Duration::from_millis(20));
    }
}

impl server::StoresServerSessions for ServerCacheWithResumptionDelay {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        let mut ssv = ServerSessionValue::read_bytes(&value).unwrap();
        match &mut ssv {
            ServerSessionValue::Tls12(tls12) => &mut tls12.common,
            ServerSessionValue::Tls13(tls13) => &mut tls13.common,
            _ => todo!(),
        }
        .creation_time_sec -= self.delay as u64;

        self.storage
            .put(key, ssv.get_encoding())
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(key)
    }

    fn take(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.take(key)
    }

    fn can_cache(&self) -> bool {
        self.storage.can_cache()
    }
}

fn make_server_cfg(opts: &Options, key_log: &Arc<KeyLogMemo>) -> Arc<ServerConfig> {
    let client_auth =
        if opts.verify_peer || opts.offer_no_client_cas || opts.require_any_client_cert {
            Arc::new(DummyClientAuth::new(
                &opts.trusted_cert_file,
                opts.require_any_client_cert,
                Arc::from(opts.root_hint_subjects.clone()),
            ))
        } else {
            WebPkiClientVerifier::no_client_auth()
        };

    assert!(
        opts.credentials.additional.is_empty(),
        "TODO: server certificate switching not implemented yet"
    );
    let cred = &opts.credentials.default;
    let (certs, key) = cred.load_from_file();

    let mut provider = opts.provider.clone();

    if let Some(groups) = &opts.groups {
        provider
            .kx_groups
            .retain(|kxg| groups.contains(&kxg.name()));
    }

    let mut cfg = ServerConfig::builder_with_provider(provider.into())
        .with_protocol_versions(&opts.supported_versions())
        .unwrap()
        .with_client_cert_verifier(client_auth)
        .with_single_cert_with_ocsp(certs, key, opts.server_ocsp_response.clone())
        .unwrap();

    cfg.session_storage = ServerCacheWithResumptionDelay::new(opts.resumption_delay);
    cfg.max_fragment_size = opts.max_fragment;
    cfg.send_tls13_tickets = 1;
    cfg.require_ems = opts.require_ems;
    cfg.ignore_client_order = opts.server_preference;
    if opts.export_traffic_secrets {
        cfg.key_log = key_log.clone();
    }

    if let Some(scheme) = cred.use_signing_scheme {
        let scheme = lookup_scheme(scheme);
        cfg.cert_resolver = Arc::new(FixedSignatureSchemeServerCertResolver {
            resolver: cfg.cert_resolver.clone(),
            scheme,
        });
    }

    if opts.tickets {
        cfg.ticketer = opts.selected_provider.ticketer();
    } else if opts.resumes == 0 {
        cfg.session_storage = Arc::new(server::NoServerSessionStorage {});
    }

    if !opts.protocols.is_empty() {
        cfg.alpn_protocols = opts
            .protocols
            .iter()
            .map(|proto| proto.as_bytes().to_vec())
            .collect::<Vec<_>>();
    }

    if opts.reject_alpn {
        cfg.alpn_protocols = vec![b"invalid".to_vec()];
    }

    if opts.enable_early_data {
        // see kMaxEarlyDataAccepted in boringssl, which bogo validates
        cfg.max_early_data_size = 14336;
        cfg.send_half_rtt_data = true;
    }

    match opts.install_cert_compression_algs {
        CompressionAlgs::All => {
            cfg.cert_compressors = vec![&ExpandingAlgorithm, &ShrinkingAlgorithm, &RandomAlgorithm];
            cfg.cert_decompressors =
                vec![&ExpandingAlgorithm, &ShrinkingAlgorithm, &RandomAlgorithm];
        }
        CompressionAlgs::One(ShrinkingAlgorithm::ALGORITHM) => {
            cfg.cert_compressors = vec![&ShrinkingAlgorithm];
            cfg.cert_decompressors = vec![&ShrinkingAlgorithm];
        }
        CompressionAlgs::None => {}
        _ => unimplemented!(),
    }

    Arc::new(cfg)
}

struct ClientCacheWithoutKxHints {
    delay: u32,
    storage: Arc<client::ClientSessionMemoryCache>,
}

impl ClientCacheWithoutKxHints {
    fn new(delay: u32) -> Arc<Self> {
        Arc::new(Self {
            delay,
            storage: Arc::new(client::ClientSessionMemoryCache::new(32)),
        })
    }
}

impl client::ClientSessionStore for ClientCacheWithoutKxHints {
    fn set_kx_hint(&self, _: ServerName<'static>, _: NamedGroup) {}
    fn kx_hint(&self, _: &ServerName<'_>) -> Option<NamedGroup> {
        None
    }

    fn set_tls12_session(
        &self,
        server_name: ServerName<'static>,
        mut value: client::Tls12ClientSessionValue,
    ) {
        value.rewind_epoch(self.delay);
        self.storage
            .set_tls12_session(server_name, value);
    }

    fn tls12_session(
        &self,
        server_name: &ServerName<'_>,
    ) -> Option<client::Tls12ClientSessionValue> {
        self.storage.tls12_session(server_name)
    }

    fn remove_tls12_session(&self, server_name: &ServerName<'static>) {
        self.storage
            .remove_tls12_session(server_name);
    }

    fn insert_tls13_ticket(
        &self,
        server_name: ServerName<'static>,
        mut value: client::Tls13ClientSessionValue,
    ) {
        value.rewind_epoch(self.delay);
        self.storage
            .insert_tls13_ticket(server_name, value)
    }

    fn take_tls13_ticket(
        &self,
        server_name: &ServerName<'static>,
    ) -> Option<client::Tls13ClientSessionValue> {
        self.storage
            .take_tls13_ticket(server_name)
    }
}

impl Debug for ClientCacheWithoutKxHints {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        // Note: we omit self.storage here as it may contain sensitive data.
        f.debug_struct("ClientCacheWithoutKxHints")
            .field("delay", &self.delay)
            .finish()
    }
}

fn make_client_cfg(opts: &Options, key_log: &Arc<KeyLogMemo>) -> Arc<ClientConfig> {
    let mut provider = opts.provider.clone();

    if let Some(groups) = &opts.groups {
        provider
            .kx_groups
            .retain(|kxg| groups.contains(&kxg.name()));
    }

    let provider = Arc::new(provider);
    let cfg = ClientConfig::builder_with_provider(provider.clone());

    let cfg = if opts.selected_provider.supports_ech() {
        if let Some(ech_config_list) = &opts.ech_config_list {
            let ech_mode: EchMode = EchConfig::new(ech_config_list.clone(), ALL_HPKE_SUITES)
                .unwrap_or_else(|_| quit(":INVALID_ECH_CONFIG_LIST:"))
                .into();

            cfg.with_ech(ech_mode)
                .expect("invalid ECH config")
        } else if opts.enable_ech_grease {
            let ech_mode = EchMode::Grease(EchGreaseConfig::new(
                GREASE_HPKE_SUITE,
                HpkePublicKey(GREASE_25519_PUBKEY.to_vec()),
            ));

            cfg.with_ech(ech_mode)
                .expect("invalid GREASE ECH config")
        } else {
            cfg.with_protocol_versions(&opts.supported_versions())
                .expect("inconsistent settings")
        }
    } else {
        cfg.with_protocol_versions(&opts.supported_versions())
            .expect("inconsistent settings")
    };

    let cfg = cfg
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(DummyServerAuth::new(
            &opts.trusted_cert_file,
            opts.ocsp,
        )));

    let mut cfg = match opts.credentials.configured() {
        true => {
            let mut resolver = MultipleClientCredentialResolver {
                expect_selected: opts.credentials.expect_selected,
                ..Default::default()
            };

            if opts.credentials.default.configured() {
                let cred = &opts.credentials.default;
                let (certs, key) = cred.load_from_file();
                let key = provider
                    .key_provider
                    .load_private_key(key)
                    .expect("cannot load private key");

                resolver.set_default(
                    sign::CertifiedKey::new(certs, key).expect("keys match"),
                    cred,
                )
            }

            for cred in opts.credentials.additional.iter() {
                let (certs, key) = cred.load_from_file();
                let key = provider
                    .key_provider
                    .load_private_key(key)
                    .expect("cannot load private key");

                resolver.add(
                    sign::CertifiedKey::new(certs, key).expect("keys match"),
                    cred,
                );
            }

            cfg.with_client_cert_resolver(Arc::new(resolver))
        }
        false => cfg.with_no_client_auth(),
    };

    cfg.resumption = Resumption::store(ClientCacheWithoutKxHints::new(opts.resumption_delay))
        .tls12_resumption(match opts.tickets {
            true => Tls12Resumption::SessionIdOrTickets,
            false => Tls12Resumption::SessionIdOnly,
        });
    cfg.enable_sni = opts.use_sni;
    cfg.max_fragment_size = opts.max_fragment;
    cfg.require_ems = opts.require_ems;
    if opts.export_traffic_secrets {
        cfg.key_log = key_log.clone();
    }

    if !opts.protocols.is_empty() {
        cfg.alpn_protocols = opts
            .protocols
            .iter()
            .map(|proto| proto.as_bytes().to_vec())
            .collect();
    }

    if opts.enable_early_data {
        cfg.enable_early_data = true;
    }

    match opts.install_cert_compression_algs {
        CompressionAlgs::All => {
            cfg.cert_decompressors =
                vec![&ExpandingAlgorithm, &ShrinkingAlgorithm, &RandomAlgorithm];
            cfg.cert_compressors = vec![&ExpandingAlgorithm, &ShrinkingAlgorithm, &RandomAlgorithm];
        }
        CompressionAlgs::One(ShrinkingAlgorithm::ALGORITHM) => {
            cfg.cert_decompressors = vec![&ShrinkingAlgorithm];
            cfg.cert_compressors = vec![&ShrinkingAlgorithm];
        }
        CompressionAlgs::None => {}
        _ => unimplemented!(),
    }

    Arc::new(cfg)
}

fn quit(why: &str) -> ! {
    println_err!("{}", why);
    process::exit(0)
}

fn quit_err(why: &str) -> ! {
    println_err!("{}", why);
    process::exit(1)
}

fn handle_err(opts: &Options, err: Error) -> ! {
    println!("TLS error: {err:?}");

    match err {
        Error::InappropriateHandshakeMessage { .. } | Error::InappropriateMessage { .. } => {
            quit(":UNEXPECTED_MESSAGE:")
        }
        Error::AlertReceived(AlertDescription::RecordOverflow) => {
            quit(":TLSV1_ALERT_RECORD_OVERFLOW:")
        }
        Error::AlertReceived(AlertDescription::HandshakeFailure) => quit(":HANDSHAKE_FAILURE:"),
        Error::AlertReceived(AlertDescription::ProtocolVersion) => quit(":WRONG_VERSION:"),
        Error::AlertReceived(AlertDescription::InternalError) => {
            quit(":PEER_ALERT_INTERNAL_ERROR:")
        }
        Error::InvalidMessage(
            InvalidMessage::MissingData("AlertDescription")
            | InvalidMessage::TrailingData("AlertMessagePayload"),
        ) => quit(":BAD_ALERT:"),
        Error::InvalidMessage(
            InvalidMessage::TrailingData("ChangeCipherSpecPayload") | InvalidMessage::InvalidCcs,
        ) => quit(":BAD_CHANGE_CIPHER_SPEC:"),
        Error::InvalidMessage(
            InvalidMessage::EmptyTicketValue | InvalidMessage::IllegalEmptyList(_),
        ) => quit(":DECODE_ERROR:"),
        Error::InvalidMessage(InvalidMessage::IllegalEmptyValue) => quit(":ILLEGAL_EMPTY_VALUE:"),
        Error::InvalidMessage(
            InvalidMessage::InvalidKeyUpdate
            | InvalidMessage::MissingData(_)
            | InvalidMessage::TrailingData(_)
            | InvalidMessage::UnexpectedMessage("HelloRetryRequest")
            | InvalidMessage::NoSignatureSchemes
            | InvalidMessage::UnsupportedCompression,
        ) => quit(":BAD_HANDSHAKE_MSG:"),
        Error::InvalidMessage(InvalidMessage::InvalidCertRequest)
        | Error::InvalidMessage(InvalidMessage::InvalidDhParams)
        | Error::InvalidMessage(InvalidMessage::MissingKeyExchange) => quit(":BAD_HANDSHAKE_MSG:"),
        Error::InvalidMessage(InvalidMessage::InvalidContentType)
        | Error::InvalidMessage(InvalidMessage::InvalidEmptyPayload)
        | Error::InvalidMessage(InvalidMessage::UnknownProtocolVersion)
        | Error::InvalidMessage(InvalidMessage::MessageTooLarge) => quit(":GARBAGE:"),
        Error::InvalidMessage(InvalidMessage::MessageTooShort)
            if opts.enable_ech_grease || opts.ech_config_list.is_some() =>
        {
            quit(":ERROR_PARSING_EXTENSION:")
        }
        Error::InvalidMessage(InvalidMessage::DuplicateExtension(_)) => {
            quit(":DUPLICATE_EXTENSION:")
        }
        Error::InvalidMessage(InvalidMessage::UnknownHelloRetryRequestExtension)
        | Error::InvalidMessage(InvalidMessage::UnknownCertificateExtension) => {
            quit(":UNEXPECTED_EXTENSION:")
        }
        Error::InvalidMessage(InvalidMessage::UnexpectedMessage(_)) => quit(":GARBAGE:"),
        Error::InvalidMessage(InvalidMessage::PreSharedKeyIsNotFinalExtension) => {
            quit(":PRE_SHARED_KEY_MUST_BE_LAST:")
        }
        Error::DecryptError if opts.ech_config_list.is_some() => {
            quit(":INCONSISTENT_ECH_NEGOTIATION:")
        }
        Error::DecryptError => quit(":DECRYPTION_FAILED_OR_BAD_RECORD_MAC:"),
        Error::NoApplicationProtocol => quit(":NO_APPLICATION_PROTOCOL:"),
        Error::PeerIncompatible(
            PeerIncompatible::ServerSentHelloRetryRequestWithUnknownExtension,
        ) => quit(":UNEXPECTED_EXTENSION:"),
        Error::RejectedEch(rejected_err) => {
            if let Some(expected_configs) = &opts.expect_ech_retry_configs {
                assert_eq!(
                    rejected_err.retry_configs().as_ref(),
                    Some(expected_configs)
                );
            }
            quit(":ECH_REJECTED:")
        }
        Error::PeerIncompatible(_) => quit(":INCOMPATIBLE:"),
        Error::PeerMisbehaved(PeerMisbehaved::MissingPskModesExtension) => {
            quit(":MISSING_EXTENSION:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::TooMuchEarlyDataReceived) => {
            quit(":TOO_MUCH_READ_EARLY_DATA:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme)
        | Error::PeerMisbehaved(PeerMisbehaved::SignedKxWithWrongAlgorithm) => {
            quit(":WRONG_SIGNATURE_TYPE:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::SelectedUnofferedCertCompression) => {
            quit(":UNKNOWN_CERT_COMPRESSION_ALG:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::InvalidCertCompression) => {
            quit(":CERT_DECOMPRESSION_FAILED:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::OfferedDuplicateCertificateCompressions) => {
            quit(":ERROR_PARSING_EXTENSION:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::SelectedUnofferedCipherSuite) => {
            quit(":WRONG_CIPHER_RETURNED:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::TooManyWarningAlertsReceived) => {
            quit(":TOO_MANY_WARNING_ALERTS:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::TooManyKeyUpdateRequests) => {
            quit(":TOO_MANY_KEY_UPDATES:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::ServerEchoedCompatibilitySessionId) => {
            quit(":SERVER_ECHOED_INVALID_SESSION_ID:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::TooManyEmptyFragments) => {
            quit(":TOO_MANY_EMPTY_FRAGMENTS:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::IllegalHelloRetryRequestWithInvalidEch)
        | Error::PeerMisbehaved(PeerMisbehaved::UnsolicitedEchExtension) => {
            quit(":UNEXPECTED_EXTENSION:")
        }
        // The TLS-ECH-Client-UnsolicitedInnerServerNameAck test is expected to fail with
        // :UNEXPECTED_EXTENSION: when we receive an unsolicited inner hello SNI extension.
        // We treat this the same as any unexpected enc'd ext and return :PEER_MISBEHAVIOUR:.
        // Convert to the expected if this error occurs when we're configured w/ ECH.
        Error::PeerMisbehaved(PeerMisbehaved::UnsolicitedEncryptedExtension)
            if opts.ech_config_list.is_some() =>
        {
            quit(":UNEXPECTED_EXTENSION:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::SelectedUnofferedKxGroup) => quit(":WRONG_CURVE:"),
        Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare) => quit(":BAD_ECPOINT:"),
        Error::PeerMisbehaved(PeerMisbehaved::MessageInterleavedWithHandshakeMessage) => {
            quit(":UNEXPECTED_MESSAGE:")
        }
        Error::PeerMisbehaved(PeerMisbehaved::KeyEpochWithPendingFragment) => {
            quit(":EXCESS_HANDSHAKE_DATA:")
        }
        Error::PeerMisbehaved(_) => quit(":PEER_MISBEHAVIOUR:"),
        Error::NoCertificatesPresented => quit(":NO_CERTS:"),
        Error::AlertReceived(AlertDescription::UnexpectedMessage) => quit(":BAD_ALERT:"),
        Error::AlertReceived(AlertDescription::DecompressionFailure) => {
            quit_err(":SSLV3_ALERT_DECOMPRESSION_FAILURE:")
        }
        Error::InvalidCertificate(CertificateError::BadEncoding) => {
            quit(":CANNOT_PARSE_LEAF_CERT:")
        }
        Error::InvalidCertificate(CertificateError::BadSignature) => quit(":BAD_SIGNATURE:"),
        #[allow(deprecated)]
        Error::InvalidCertificate(
            CertificateError::UnsupportedSignatureAlgorithm
            | CertificateError::UnsupportedSignatureAlgorithmContext { .. }
            | CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext { .. },
        ) => quit(":WRONG_SIGNATURE_TYPE:"),
        Error::InvalidCertificate(CertificateError::InvalidOcspResponse) => {
            // note: only use is in this file.
            quit(":OCSP_CB_ERROR:")
        }
        Error::InvalidCertificate(e) => quit(&format!(":BAD_CERT: ({e:?})")),
        Error::PeerSentOversizedRecord => quit(":DATA_LENGTH_TOO_LONG:"),
        _ => {
            println_err!("unhandled error: {:?}", err);
            quit(":FIXME:")
        }
    }
}

fn flush(sess: &mut Connection, conn: &mut net::TcpStream) {
    while sess.wants_write() {
        if let Err(err) = sess.write_tls(conn) {
            println!("IO error: {err:?}");
            process::exit(0);
        }
    }
    conn.flush().unwrap();
}

fn client(conn: &mut Connection) -> &mut ClientConnection {
    conn.try_into().unwrap()
}

fn server(conn: &mut Connection) -> &mut ServerConnection {
    match conn {
        Connection::Server(s) => s,
        _ => panic!("Connection is not a ServerConnection"),
    }
}

const MAX_MESSAGE_SIZE: usize = 0xffff + 5;

fn after_read(opts: &Options, sess: &mut Connection, conn: &mut net::TcpStream) {
    if let Err(err) = sess.process_new_packets() {
        flush(sess, conn); /* send any alerts before exiting */
        orderly_close(conn);
        handle_err(opts, err);
    }
}

fn orderly_close(conn: &mut net::TcpStream) {
    // assuming we just flush()'d, we will write no more.
    conn.shutdown(net::Shutdown::Write)
        .unwrap();

    // wait for EOF
    let mut buf = [0u8; 32];
    while let Ok(p @ 1..) = conn.peek(&mut buf) {
        let _ = conn.read(&mut buf[..p]).unwrap();
    }

    let _ = conn.shutdown(net::Shutdown::Read);
}

fn read_n_bytes(opts: &Options, sess: &mut Connection, conn: &mut net::TcpStream, n: usize) {
    let mut bytes = [0u8; MAX_MESSAGE_SIZE];
    match conn.read(&mut bytes[..n]) {
        Ok(count) => {
            println!("read {count:?} bytes");
            sess.read_tls(&mut io::Cursor::new(&mut bytes[..count]))
                .expect("read_tls not expected to fail reading from buffer");
        }
        Err(err) if err.kind() == io::ErrorKind::ConnectionReset => {}
        Err(err) => panic!("invalid read: {err}"),
    };

    after_read(opts, sess, conn);
}

fn read_all_bytes(opts: &Options, sess: &mut Connection, conn: &mut net::TcpStream) {
    match sess.read_tls(conn) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::ConnectionReset => {}
        Err(err) => panic!("invalid read: {err}"),
    };

    after_read(opts, sess, conn);
}

fn exec(opts: &Options, mut sess: Connection, key_log: &KeyLogMemo, count: usize) {
    let mut sent_message = false;

    let addrs = [
        net::SocketAddr::from((net::Ipv6Addr::LOCALHOST, opts.port)),
        net::SocketAddr::from((net::Ipv4Addr::LOCALHOST, opts.port)),
    ];
    let mut conn = net::TcpStream::connect(&addrs[..]).expect("cannot connect");
    let mut sent_shutdown = false;
    let mut sent_exporter = false;
    let mut sent_key_update = false;
    let mut quench_writes = false;

    conn.write_all(&opts.shim_id.to_le_bytes())
        .unwrap();

    loop {
        if !sent_message && (opts.queue_data || (opts.queue_data_on_resume && count > 0)) {
            if !opts
                .queue_early_data_after_received_messages
                .is_empty()
            {
                flush(&mut sess, &mut conn);
                for message_size_estimate in &opts.queue_early_data_after_received_messages {
                    read_n_bytes(opts, &mut sess, &mut conn, *message_size_estimate);
                }
                println!("now ready for early data");
            }

            if count > 0 && opts.enable_early_data {
                let len = client(&mut sess)
                    .early_data()
                    .expect("0rtt not available")
                    .write(b"hello")
                    .expect("0rtt write failed");
                sess.writer()
                    .write_all(&b"hello"[len..])
                    .unwrap();
                sent_message = true;
            } else if !opts.only_write_one_byte_after_handshake {
                let _ = sess.writer().write_all(b"hello");
                sent_message = true;
            }
        }

        if !quench_writes {
            flush(&mut sess, &mut conn);
        }

        if sess.wants_read() {
            read_all_bytes(opts, &mut sess, &mut conn);
        }

        if opts.side == Side::Server && opts.enable_early_data {
            if let Some(ed) = &mut server(&mut sess).early_data() {
                let mut data = Vec::new();
                let data_len = ed
                    .read_to_end(&mut data)
                    .expect("cannot read early_data");

                for b in data.iter_mut() {
                    *b ^= 0xff;
                }

                sess.writer()
                    .write_all(&data[..data_len])
                    .expect("cannot echo early_data in 1rtt data");
            }
        }

        if !sess.is_handshaking() && opts.export_keying_material > 0 && !sent_exporter {
            let mut export = vec![0; opts.export_keying_material];
            sess.export_keying_material(
                &mut export,
                opts.export_keying_material_label
                    .as_bytes(),
                if opts.export_keying_material_context_used {
                    Some(
                        opts.export_keying_material_context
                            .as_bytes(),
                    )
                } else {
                    None
                },
            )
            .unwrap();
            sess.writer()
                .write_all(&export)
                .unwrap();
            sent_exporter = true;
        }

        if !sess.is_handshaking() && opts.export_traffic_secrets && !sent_exporter {
            let secrets = key_log.clone_inner();
            assert_eq!(
                secrets.client_traffic_secret.len(),
                secrets.server_traffic_secret.len()
            );
            sess.writer()
                .write_all(&(secrets.client_traffic_secret.len() as u16).to_le_bytes())
                .unwrap();
            sess.writer()
                .write_all(&secrets.server_traffic_secret)
                .unwrap();
            sess.writer()
                .write_all(&secrets.client_traffic_secret)
                .unwrap();
            sent_exporter = true;
        }

        if opts.send_key_update && !sent_key_update && !sess.is_handshaking() {
            sess.refresh_traffic_keys().unwrap();
            sent_key_update = true;
        }

        if !sess.is_handshaking() && opts.only_write_one_byte_after_handshake && !sent_message {
            println!("writing message and then only one byte of its tls frame");
            flush(&mut sess, &mut conn);

            sess.writer()
                .write_all(b"hello")
                .unwrap();
            sent_message = true;

            let mut one_byte = [0u8];
            let mut cursor = io::Cursor::new(&mut one_byte[..]);
            sess.write_tls(&mut cursor).unwrap();
            conn.write_all(&one_byte)
                .expect("IO error");

            quench_writes = true;
        }

        if opts.enable_early_data
            && opts.side == Side::Client
            && !sess.is_handshaking()
            && count > 0
        {
            if opts.expect_accept_early_data && !client(&mut sess).is_early_data_accepted() {
                quit_err("Early data was not accepted, but we expect the opposite");
            } else if opts.expect_reject_early_data && client(&mut sess).is_early_data_accepted() {
                quit_err("Early data was accepted, but we expect the opposite");
            }
            if opts.expect_version == 0x0304 {
                match sess.protocol_version() {
                    Some(ProtocolVersion::TLSv1_3) | Some(ProtocolVersion::Unknown(0x7f17)) => {}
                    _ => quit_err("wrong protocol version"),
                }
            }
        }

        if let (Some(expected_options), false) =
            (opts.expect_handshake_kind.as_ref(), sess.is_handshaking())
        {
            let actual = sess.handshake_kind().unwrap();
            assert!(
                expected_options.contains(&actual),
                "wanted to see {expected_options:?} but got {actual:?}"
            );
        }

        if let Some(curve_id) = &opts.expect_curve_id {
            // unlike openssl/boringssl's API, `negotiated_key_exchange_group`
            // works for the connection, not session.  this means TLS1.2
            // resumptions never have a value for `negotiated_key_exchange_group`
            let tls12_resumed = sess.protocol_version() == Some(ProtocolVersion::TLSv1_2)
                && sess.handshake_kind() == Some(HandshakeKind::Resumed);
            let negotiated_key_exchange_group_ready = !(sess.is_handshaking() || tls12_resumed);

            if negotiated_key_exchange_group_ready {
                let actual = sess
                    .negotiated_key_exchange_group()
                    .expect("no kx with -expect-curve-id");
                assert_eq!(curve_id, &actual.name());
            }
        }

        if let Some(curve_id) = &opts.on_initial_expect_curve_id {
            if !sess.is_handshaking() && count == 0 {
                assert_eq!(sess.handshake_kind().unwrap(), HandshakeKind::Full);
                assert_eq!(
                    sess.negotiated_key_exchange_group()
                        .expect("no kx with -on-initial-expect-curve-id")
                        .name(),
                    *curve_id
                );
            }
        }

        if let Some(curve_id) = &opts.on_resume_expect_curve_id {
            if !sess.is_handshaking() && count > 0 {
                assert_eq!(sess.handshake_kind().unwrap(), HandshakeKind::Resumed);
                assert_eq!(
                    sess.negotiated_key_exchange_group()
                        .expect("no kx with -on-resume-expect-curve-id")
                        .name(),
                    *curve_id
                );
            }
        }

        {
            let ech_accept_required =
                (count == 0 && opts.on_initial_expect_ech_accept) || opts.expect_ech_accept;
            if ech_accept_required
                && !sess.is_handshaking()
                && client(&mut sess).ech_status() != EchStatus::Accepted
            {
                quit_err("ECH was not accepted, but we expect the opposite");
            }
        }

        let mut buf = [0u8; 1024];
        let len = match sess
            .reader()
            .read(&mut buf[..opts.read_size])
        {
            Ok(0) => {
                if opts.check_close_notify {
                    println!("close notify ok");
                }
                println!("EOF (tls)");
                return;
            }
            Ok(len) => len,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => 0,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                if opts.check_close_notify {
                    quit_err(":CLOSE_WITHOUT_CLOSE_NOTIFY:");
                }
                println!("EOF (tcp)");
                return;
            }
            Err(err) => panic!("unhandled read error {err:?}"),
        };

        if opts.shut_down_after_handshake && !sent_shutdown && !sess.is_handshaking() {
            sess.send_close_notify();
            sent_shutdown = true;
        }

        if quench_writes && len > 0 {
            println!("unquenching writes after {len:?}");
            quench_writes = false;
        }

        for b in buf.iter_mut() {
            *b ^= 0xff;
        }

        sess.writer()
            .write_all(&buf[..len])
            .unwrap();
    }
}

pub fn main() {
    let mut args: Vec<_> = env::args().collect();
    env_logger::init();

    args.remove(0);

    if !args.is_empty() && args[0] == "-is-handshaker-supported" {
        println!("No");
        process::exit(0);
    }
    println!("options: {args:?}");

    let mut opts = Options::new();

    while !args.is_empty() {
        let arg = args.remove(0);
        match arg.as_ref() {
            "-port" => {
                opts.port = args.remove(0).parse::<u16>().unwrap();
            }
            "-shim-id" => {
                opts.shim_id = args.remove(0).parse::<u64>().unwrap();
            }
            "-server" => {
                opts.side = Side::Server;
            }
            "-key-file" => {
                opts.credentials.last_mut().key_file = args.remove(0);
            }
            "-new-x509-credential" => {
                opts.credentials.additional.push(Credential::default());
            }
            "-expect-selected-credential" => {
                opts.credentials.expect_selected = args.remove(0).parse::<isize>().ok();
            }
            "-cert-file" => {
                opts.credentials.last_mut().cert_file = args.remove(0);
            }
            "-trust-cert" => {
                opts.trusted_cert_file = args.remove(0);
            }
            "-resume-count" => {
                opts.resumes = args.remove(0).parse::<usize>().unwrap();
            }
            "-no-tls13" => {
                opts.support_tls13 = false;
            }
            "-no-tls12" => {
                opts.support_tls12 = false;
            }
            "-min-version" => {
                let min = args.remove(0).parse::<u16>().unwrap();
                opts.min_version = Some(ProtocolVersion::Unknown(min));
            }
            "-max-version" => {
                let max = args.remove(0).parse::<u16>().unwrap();
                opts.max_version = Some(ProtocolVersion::Unknown(max));
            }
            "-max-send-fragment" => {
                let max_fragment = args.remove(0).parse::<usize>().unwrap();
                opts.max_fragment = Some(max_fragment + 5); // ours includes header
            }
            "-read-size" => {
                let rdsz = args.remove(0).parse::<usize>().unwrap();
                opts.read_size = rdsz;
            }
            "-tls13-variant" => {
                let variant = args.remove(0).parse::<u16>().unwrap();
                if variant != 1 {
                    println!("NYI TLS1.3 variant selection: {arg:?} {variant:?}");
                    process::exit(BOGO_NACK);
                }
            }
            "-no-ticket" => {
                opts.tickets = false;
            }
            "-on-resume-no-ticket" => {
                opts.resume_with_tickets_disabled = true;
            }
            "-signing-prefs" => {
                let alg = args.remove(0).parse::<u16>().unwrap();
                opts.credentials.last_mut().use_signing_scheme = Some(alg);
            }
            "-must-match-issuer" => {
                opts.credentials.last_mut().must_match_issuer = true;
            }
            "-use-client-ca-list" => {
                match args.remove(0).as_ref() {
                    "<EMPTY>" | "<NULL>" => {
                        opts.root_hint_subjects = vec![];
                    }
                    list => {
                        opts.root_hint_subjects = list.split(',')
                            .map(|entry| DistinguishedName::from(decode_hex(entry)))
                            .collect();
                    }
                }
            }
            "-verify-prefs" => {
                lookup_scheme(args.remove(0).parse::<u16>().unwrap());
            }
            "-expect-curve-id" => {
                opts.expect_curve_id = Some(NamedGroup::from(args.remove(0).parse::<u16>().unwrap()));
            }
            "-on-initial-expect-curve-id" => {
                opts.on_initial_expect_curve_id = Some(NamedGroup::from(args.remove(0).parse::<u16>().unwrap()));
            }
            "-on-resume-expect-curve-id" => {
                opts.on_resume_expect_curve_id = Some(NamedGroup::from(args.remove(0).parse::<u16>().unwrap()));
            }
            "-max-cert-list" |
            "-expect-peer-signature-algorithm" |
            "-expect-peer-verify-pref" |
            "-expect-advertised-alpn" |
            "-expect-alpn" |
            "-on-initial-expect-alpn" |
            "-on-resume-expect-alpn" |
            "-on-retry-expect-alpn" |
            "-expect-server-name" |
            "-expect-ocsp-response" |
            "-expect-signed-cert-timestamps" |
            "-expect-certificate-types" |
            "-expect-client-ca-list" |
            "-on-retry-expect-early-data-reason" |
            "-on-resume-expect-early-data-reason" |
            "-on-initial-expect-early-data-reason" |
            "-on-initial-expect-cipher" |
            "-on-resume-expect-cipher" |
            "-on-retry-expect-cipher" |
            "-expect-ticket-age-skew" |
            "-handshaker-path" |
            "-application-settings" |
            "-expect-msg-callback" => {
                println!("not checking {} {}; NYI", arg, args.remove(0));
            }

            "-expect-secure-renegotiation" |
            "-expect-no-session-id" |
            "-enable-ed25519" |
            "-on-resume-expect-no-offer-early-data" |
            "-expect-tls13-downgrade" |
            "-enable-signed-cert-timestamps" |
            "-expect-session-id" => {
                println!("not checking {arg}; NYI");
            }

            "-key-update" => {
                opts.send_key_update = true;
            }
            "-expect-hrr" => {
                opts.expect_handshake_kind = Some(vec![HandshakeKind::FullWithHelloRetryRequest]);
            }
            "-expect-no-hrr" => {
                opts.expect_handshake_kind = Some(vec![HandshakeKind::Full]);
            }
            "-expect-session-miss" => {
                opts.expect_handshake_kind_resumed = Some(vec![
                    HandshakeKind::Full,
                    HandshakeKind::FullWithHelloRetryRequest
                ]);
            }
            "-export-keying-material" => {
                opts.export_keying_material = args.remove(0).parse::<usize>().unwrap();
            }
            "-export-label" => {
                opts.export_keying_material_label = args.remove(0);
            }
            "-export-context" => {
                opts.export_keying_material_context = args.remove(0);
            }
            "-use-export-context" => {
                opts.export_keying_material_context_used = true;
            }
            "-export-traffic-secrets" => {
                opts.export_traffic_secrets = true;
            }
            "-quic-transport-params" => {
                opts.quic_transport_params = BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }
            "-expect-quic-transport-params" => {
                opts.expect_quic_transport_params = BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }

            "-ocsp-response" => {
                opts.server_ocsp_response = BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }
            "-select-alpn" => {
                opts.protocols.push(args.remove(0));
            }
            "-require-any-client-certificate" => {
                opts.require_any_client_cert = true;
            }
            "-verify-peer" => {
                opts.verify_peer = true;
            }
            "-shim-writes-first" => {
                opts.queue_data = true;
            }
            "-read-with-unfinished-write" => {
                opts.queue_data = true;
                opts.only_write_one_byte_after_handshake = true;
            }
            "-shim-shuts-down" => {
                opts.shut_down_after_handshake = true;
            }
            "-check-close-notify" => {
                opts.check_close_notify = true;
            }
            "-host-name" => {
                opts.host_name = args.remove(0);
                opts.use_sni = true;
            }
            "-advertise-alpn" => {
                opts.protocols = split_protocols(&args.remove(0));
            }
            "-reject-alpn" => {
                opts.reject_alpn = true;
            }
            "-use-null-client-ca-list" => {
                opts.offer_no_client_cas = true;
            }
            "-enable-early-data" => {
                opts.tickets = false;
                opts.enable_early_data = true;
            }
            "-on-resume-shim-writes-first" => {
                opts.queue_data_on_resume = true;
            }
            "-on-resume-read-with-unfinished-write" => {
                opts.queue_data_on_resume = true;
                opts.only_write_one_byte_after_handshake_on_resume = true;
            }
            "-on-resume-early-write-after-message" => {
                opts.queue_early_data_after_received_messages= match args.remove(0).parse::<u8>().unwrap() {
                    // estimate where these messages appear in the server's first flight.
                    2 => vec![5 + 128 + 5 + 32],
                    8 => vec![5 + 128 + 5 + 32, 5 + 64],
                    _ => {
                        panic!("unhandled -on-resume-early-write-after-message");
                    }
                };
                opts.queue_data_on_resume = true;
            }
            "-expect-ticket-supports-early-data" => {
                opts.expect_ticket_supports_early_data = true;
            }
            "-expect-accept-early-data" |
            "-on-resume-expect-accept-early-data" => {
                opts.expect_accept_early_data = true;
            }
            "-expect-early-data-reason" |
            "-on-resume-expect-reject-early-data-reason" => {
                let reason = args.remove(0);
                match reason.as_str() {
                    "disabled" | "protocol_version" => {
                        opts.expect_reject_early_data = true;
                    }
                    _ => {
                        println!("NYI early data reason: {reason}");
                        process::exit(1);
                    }
                }
            }
            "-expect-reject-early-data" |
            "-on-resume-expect-reject-early-data" => {
                opts.expect_reject_early_data = true;
            }
            "-expect-version" => {
                opts.expect_version = args.remove(0).parse::<u16>().unwrap();
            }
            "-curves" => {
                let group = NamedGroup::from(args.remove(0).parse::<u16>().unwrap());
                opts.groups.get_or_insert(Vec::new()).push(group);
            }
            "-resumption-delay" => {
                opts.resumption_delay = args.remove(0).parse::<u32>().unwrap();
                align_time();
            }
            "-expect-extended-master-secret" => {
                opts.require_ems = true;
            }
            "-install-cert-compression-algs" => {
                opts.install_cert_compression_algs = CompressionAlgs::All;
            }
            "-install-one-cert-compression-alg" => {
                opts.install_cert_compression_algs = CompressionAlgs::One(args.remove(0).parse::<u16>().unwrap());
            }
            #[cfg(feature = "fips")]
            "-fips-202205" if opts.selected_provider == SelectedProvider::AwsLcRsFips => {
                opts.provider = rustls::crypto::default_fips_provider();
            }
            "-fips-202205" => {
                println!("Not a FIPS build");
                process::exit(BOGO_NACK);
            }
            "-ech-config-list" => {
                opts.ech_config_list = Some(BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid ECH config base64").into());
            }
            "-expect-ech-accept" => {
                opts.expect_ech_accept = true;
            }
            "-expect-ech-retry-configs" => {
                opts.expect_ech_retry_configs = Some(BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid ECH config base64").into());
            }
            "-on-resume-ech-config-list" => {
                opts.on_resume_ech_config_list = Some(BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid on resume ECH config base64").into());
            }
            "-on-resume-expect-ech-accept" => {
                opts.on_resume_expect_ech_accept = true;
            }
            "-expect-no-ech-retry-configs" => {
                opts.expect_ech_retry_configs = None;
            }
            "-on-initial-expect-ech-accept" => {
                opts.on_initial_expect_ech_accept = true;
            }
            "-on-retry-expect-ech-retry-configs" => {
                // Note: we treat this the same as -expect-ech-retry-configs
                opts.expect_ech_retry_configs = Some(BASE64_STANDARD.decode(args.remove(0).as_bytes())
                    .expect("invalid retry ECH config base64").into());
            }
            "-enable-ech-grease" => {
                opts.enable_ech_grease = true;
            }
            "-server-preference" => {
                opts.server_preference = true;
            }
            "-fail-ocsp-callback" => {
                opts.ocsp = OcspValidation::Reject;
            }
            "-wait-for-debugger" => {
                #[cfg(windows)]
                {
                    panic("-wait-for-debugger not supported on Windows");
                }
                #[cfg(unix)]
                {
                    opts.wait_for_debugger = true;
                }
            }

            // defaults:
            "-enable-all-curves" |
            "-renegotiate-ignore" |
            "-no-tls11" |
            "-no-tls1" |
            "-no-ssl3" |
            "-handoff" |
            "-ipv6" |
            "-decline-alpn" |
            "-permute-extensions" |
            "-expect-no-session" |
            "-expect-ticket-renewal" |
            "-enable-ocsp-stapling" |
            "-use-ocsp-callback" |
            "-forbid-renegotiation-after-handshake" |
            // internal openssl details:
            "-async" |
            "-implicit-handshake" |
            "-use-old-client-cert-callback" |
            "-use-early-callback" => {}

            // Not implemented things
            "-dtls" |
            "-cipher" |
            "-psk" |
            "-renegotiate-freely" |
            "-false-start" |
            "-fallback-scsv" |
            "-fail-early-callback" |
            "-fail-cert-callback" |
            "-install-ddos-callback" |
            "-advertise-npn" |
            "-advertise-empty-npn" |
            "-verify-fail" |
            "-expect-channel-id" |
            "-send-channel-id" |
            "-select-next-proto" |
            "-select-empty-next-proto" |
            "-expect-verify-result" |
            "-send-alert" |
            "-digest-prefs" |
            "-use-exporter-between-reads" |
            "-ticket-key" |
            "-tls-unique" |
            "-enable-server-custom-extension" |
            "-enable-client-custom-extension" |
            "-expect-dhe-group-size" |
            "-use-ticket-callback" |
            "-enable-grease" |
            "-enable-channel-id" |
            "-expect-early-data-info" |
            "-expect-cipher-aes" |
            "-retain-only-sha256-client-cert-initial" |
            "-expect-draft-downgrade" |
            "-allow-unknown-alpn-protos" |
            "-on-initial-tls13-variant" |
            "-on-resume-export-early-keying-material" |
            "-on-resume-enable-early-data" |
            "-export-early-keying-material" |
            "-handshake-twice" |
            "-on-resume-verify-fail" |
            "-reverify-on-resume" |
            "-no-op-extra-handshake" |
            "-expect-peer-cert-file" |
            "-no-rsa-pss-rsae-certs" |
            "-ignore-tls13-downgrade" |
            "-allow-hint-mismatch" |
            "-wpa-202304" |
            "-cnsa-202407" |
            "-srtp-profiles" |
            "-use-ticket-aead-callback" |
            "-signed-cert-timestamps" |
            "-on-initial-expect-peer-cert-file" |
            "-resumption-across-names-enabled" |
            "-expect-resumable-across-names" |
            "-expect-not-resumable-across-names" |
            "-use-custom-verify-callback" => {
                println!("NYI option {arg:?}");
                process::exit(BOGO_NACK);
            }

            "-print-rustls-provider" => {
                println!("{}", "*".repeat(66));
                println!("rustls provider is {:?}", opts.selected_provider);
                println!("{}", "*".repeat(66));
                process::exit(0);
            }

            _ => {
                println!("unhandled option {arg:?}");
                process::exit(1);
            }
        }
    }

    println!("opts {opts:?}");

    #[cfg(unix)]
    if opts.wait_for_debugger {
        // On Unix systems when -wait-for-debugger is passed from the BoGo runner
        // we should SIGSTOP ourselves to allow a debugger to attach to the shim to
        // continue the testing process.
        signal::kill(Pid::from_raw(process::id() as i32), Signal::SIGSTOP).unwrap();
    }

    let key_log = Arc::new(KeyLogMemo::default());

    let (mut client_cfg, mut server_cfg) = match opts.side {
        Side::Client => (Some(make_client_cfg(&opts, &key_log)), None),
        Side::Server => (None, Some(make_server_cfg(&opts, &key_log))),
    };

    fn make_session(
        opts: &Options,
        scfg: &Option<Arc<ServerConfig>>,
        ccfg: &Option<Arc<ClientConfig>>,
    ) -> Connection {
        assert!(opts.quic_transport_params.is_empty());
        assert!(
            opts.expect_quic_transport_params
                .is_empty()
        );

        if opts.side == Side::Server {
            let scfg = scfg.as_ref().cloned().unwrap();
            ServerConnection::new(scfg)
                .unwrap()
                .into()
        } else {
            let server_name = ServerName::try_from(opts.host_name.as_str())
                .unwrap()
                .to_owned();
            let ccfg = ccfg.as_ref().cloned().unwrap();

            ClientConnection::new(ccfg, server_name)
                .unwrap()
                .into()
        }
    }

    for i in 0..opts.resumes + 1 {
        let sess = make_session(&opts, &server_cfg, &client_cfg);
        exec(&opts, sess, &key_log, i);
        if opts.resume_with_tickets_disabled {
            opts.tickets = false;

            match opts.side {
                Side::Server => server_cfg = Some(make_server_cfg(&opts, &key_log)),
                Side::Client => client_cfg = Some(make_client_cfg(&opts, &key_log)),
            };
        }
        if opts.on_resume_ech_config_list.is_some() {
            opts.ech_config_list
                .clone_from(&opts.on_resume_ech_config_list);
            opts.expect_ech_accept = opts.on_resume_expect_ech_accept;
            client_cfg = Some(make_client_cfg(&opts, &key_log));
        }
        opts.expect_handshake_kind
            .clone_from(&opts.expect_handshake_kind_resumed);
    }
}

#[derive(Debug, Default)]
struct KeyLogMemo(Mutex<KeyLogMemoInner>);

impl KeyLogMemo {
    fn clone_inner(&self) -> KeyLogMemoInner {
        self.0.lock().unwrap().clone()
    }
}

impl rustls::KeyLog for KeyLogMemo {
    fn log(&self, label: &str, _client_random: &[u8], secret: &[u8]) {
        match label {
            "CLIENT_TRAFFIC_SECRET_0" => {
                self.0
                    .lock()
                    .unwrap()
                    .client_traffic_secret = secret.to_vec()
            }
            "SERVER_TRAFFIC_SECRET_0" => {
                self.0
                    .lock()
                    .unwrap()
                    .server_traffic_secret = secret.to_vec()
            }
            _ => {}
        }
    }

    fn will_log(&self, _label: &str) -> bool {
        true
    }
}

#[derive(Clone, Debug, Default)]
struct KeyLogMemoInner {
    client_traffic_secret: Vec<u8>,
    server_traffic_secret: Vec<u8>,
}

#[derive(Debug, PartialEq)]
enum CompressionAlgs {
    None,
    All,
    One(u16),
}

#[derive(Debug)]
struct ShrinkingAlgorithm;

impl ShrinkingAlgorithm {
    const ALGORITHM: u16 = 0xff01;
}

impl compress::CertDecompressor for ShrinkingAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(Self::ALGORITHM)
    }

    fn decompress(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), compress::DecompressionFailed> {
        if output.len() != input.len() + 2 {
            return Err(compress::DecompressionFailed);
        }
        output[..2].copy_from_slice(&[0, 0]);
        output[2..].copy_from_slice(input);
        Ok(())
    }
}

impl compress::CertCompressor for ShrinkingAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(Self::ALGORITHM)
    }

    fn compress(
        &self,
        mut input: Vec<u8>,
        _: compress::CompressionLevel,
    ) -> Result<Vec<u8>, compress::CompressionFailed> {
        assert_eq!(input[..2], [0, 0]);
        input.drain(0..2);
        Ok(input)
    }
}

#[derive(Debug)]
struct ExpandingAlgorithm;

impl compress::CertDecompressor for ExpandingAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(0xff02)
    }

    fn decompress(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), compress::DecompressionFailed> {
        if output.len() + 4 != input.len() {
            return Err(compress::DecompressionFailed);
        }
        if input[..4] != [1, 2, 3, 4] {
            return Err(compress::DecompressionFailed);
        }
        output.copy_from_slice(&input[4..]);
        Ok(())
    }
}

impl compress::CertCompressor for ExpandingAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(0xff02)
    }

    fn compress(
        &self,
        mut input: Vec<u8>,
        _: compress::CompressionLevel,
    ) -> Result<Vec<u8>, compress::CompressionFailed> {
        input.insert(0, 1);
        input.insert(1, 2);
        input.insert(2, 3);
        input.insert(3, 4);
        Ok(input)
    }
}

#[derive(Debug)]
struct RandomAlgorithm;

impl compress::CertDecompressor for RandomAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(0xff03)
    }

    fn decompress(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), compress::DecompressionFailed> {
        if output.len() + 1 != input.len() {
            return Err(compress::DecompressionFailed);
        }
        output.copy_from_slice(&input[1..]);
        Ok(())
    }
}

impl compress::CertCompressor for RandomAlgorithm {
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::Unknown(0xff03)
    }

    fn compress(
        &self,
        mut input: Vec<u8>,
        _: compress::CompressionLevel,
    ) -> Result<Vec<u8>, compress::CompressionFailed> {
        let random_byte = {
            let mut bytes = [0];
            // nb. provider is irrelevant for this use
            ring::default_provider()
                .secure_random
                .fill(&mut bytes)
                .unwrap();
            bytes[0]
        };
        input.insert(0, random_byte);
        Ok(input)
    }
}

static GREASE_HPKE_SUITE: &dyn Hpke = hpke::DH_KEM_X25519_HKDF_SHA256_AES_128;

const GREASE_25519_PUBKEY: &[u8] = &[
    0x67, 0x35, 0xCA, 0x50, 0x21, 0xFC, 0x4F, 0xE6, 0x29, 0x3B, 0x31, 0x2C, 0xB5, 0xE0, 0x97, 0xD8,
    0xD0, 0x58, 0x97, 0xCF, 0x5C, 0x15, 0x12, 0x79, 0x4B, 0xEF, 0x1D, 0x98, 0x52, 0x74, 0xDC, 0x5E,
];

// nb. hpke::ALL_SUPPORTED_SUITES omits fips-incompatible options,
// this includes them. bogo fips tests are activated by -fips-202205
// (and no ech tests use that option)
static ALL_HPKE_SUITES: &[&dyn Hpke] = &[
    hpke::DH_KEM_P256_HKDF_SHA256_AES_128,
    hpke::DH_KEM_P256_HKDF_SHA256_AES_256,
    hpke::DH_KEM_P256_HKDF_SHA256_CHACHA20_POLY1305,
    hpke::DH_KEM_P384_HKDF_SHA384_AES_128,
    hpke::DH_KEM_P384_HKDF_SHA384_AES_256,
    hpke::DH_KEM_P384_HKDF_SHA384_CHACHA20_POLY1305,
    hpke::DH_KEM_P521_HKDF_SHA512_AES_128,
    hpke::DH_KEM_P521_HKDF_SHA512_AES_256,
    hpke::DH_KEM_P521_HKDF_SHA512_CHACHA20_POLY1305,
    hpke::DH_KEM_X25519_HKDF_SHA256_AES_128,
    hpke::DH_KEM_X25519_HKDF_SHA256_AES_256,
    hpke::DH_KEM_X25519_HKDF_SHA256_CHACHA20_POLY1305,
];
