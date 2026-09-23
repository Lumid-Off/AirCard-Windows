//! App-owned pairing credentials, encrypted for the current Windows account.
//! Never overwrite Apple's global Lockdown records or invoke USBMuxSavePairingRecord.
use crate::usbmux::canonical_udid;
use anyhow::{Context, Result, ensure};
use plist::{Dictionary, Value};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, IsCa, KeyPair,
    PKCS_RSA_SHA256, PublicKeyData, SignatureAlgorithm,
};
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs1::{DecodeRsaPublicKey, EncodeRsaPublicKey},
    pkcs8::{DecodePublicKey, EncodePrivateKey},
};
use rustls::pki_types::CertificateDer;
use std::io::Cursor;
use std::path::{Path, PathBuf};

pub struct PairRecord(pub Dictionary);
impl PairRecord {
    pub fn data(&self, k: &str) -> Result<&[u8]> {
        self.0
            .get(k)
            .and_then(Value::as_data)
            .context(format!("Pair record missing {k}"))
    }
    pub fn text(&self, k: &str) -> Result<&str> {
        self.0
            .get(k)
            .and_then(Value::as_string)
            .context(format!("Pair record missing {k}"))
    }
    pub fn public_record(&self) -> Result<Value> {
        let mut d = Dictionary::new();
        for k in [
            "DeviceCertificate",
            "HostCertificate",
            "RootCertificate",
            "HostID",
            "SystemBUID",
        ] {
            d.insert(
                k.into(),
                self.0.get(k).cloned().context(format!("Missing {k}"))?,
            );
        }
        Ok(Value::Dictionary(d))
    }
    pub fn matches_device(&self, public_key: &[u8]) -> Result<bool> {
        // v1 used identical empty subject/issuer names for root and leaf.
        // Some TLS peers treat the leaf as self-signed and reject client auth.
        if self
            .0
            .get("PairingFormatVersion")
            .and_then(Value::as_unsigned_integer)
            != Some(3)
        {
            return Ok(false);
        }
        let key = parse_public_key(public_key)?;
        let cert = certificate_der(self.data("DeviceCertificate")?)?;
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
            .map_err(|_| anyhow::anyhow!("Invalid stored device certificate"))?;
        Ok(RsaPublicKey::from_public_key_der(parsed.public_key().raw)? == key)
    }
    pub fn native_tls_connector(&self) -> Result<native_tls::TlsConnector> {
        let identity = native_tls::Identity::from_pkcs8(
            self.data("HostCertificate")?,
            self.data("HostPrivateKey")?,
        )
        .context("Load paired host TLS identity")?;
        let mut builder = native_tls::TlsConnector::builder();
        builder
            .identity(identity)
            .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
            .max_protocol_version(Some(native_tls::Protocol::Tlsv12))
            .use_sni(false)
            .danger_accept_invalid_hostnames(true)
            // Apple pairing certificates have no DNS name or public CA chain.
            // secure() MUST pin the peer certificate before exposing the stream.
            .danger_accept_invalid_certs(true);
        builder
            .build()
            .context("Configure Windows TLS for paired iPhone")
    }
    pub fn verify_peer_certificate(&self, der: &[u8]) -> Result<()> {
        ensure!(
            certificate_der(self.data("DeviceCertificate")?)?.as_ref() == der,
            "iPhone certificate does not match its pairing record"
        );
        Ok(())
    }
}
fn certificate_der(pem: &[u8]) -> Result<CertificateDer<'static>> {
    rustls_pemfile::certs(&mut Cursor::new(pem))
        .next()
        .context("Missing certificate")?
        .context("Invalid certificate PEM")
}
fn parse_public_key(pem: &[u8]) -> Result<RsaPublicKey> {
    let s = std::str::from_utf8(pem)?;
    RsaPublicKey::from_pkcs1_pem(s)
        .or_else(|_| RsaPublicKey::from_public_key_pem(s))
        .context("iPhone supplied an invalid RSA public key")
}
struct DeviceKey(Vec<u8>);
impl PublicKeyData for DeviceKey {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }
    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_RSA_SHA256
    }
}
fn rsa_key() -> Result<KeyPair> {
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?;
    let doc = key.to_pkcs8_der()?;
    KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls::pki_types::PrivatePkcs8KeyDer::from(doc.as_bytes()),
        &PKCS_RSA_SHA256,
    )
    .context("Generating pairing key")
}
pub fn generate(public_key: &[u8], buid: &str) -> Result<PairRecord> {
    let device = DeviceKey(
        parse_public_key(public_key)?
            .to_pkcs1_der()?
            .as_bytes()
            .to_vec(),
    );
    let mut root_params = CertificateParams::new(Vec::<String>::new())?;
    root_params.distinguished_name = DistinguishedName::new();
    root_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "AirCard PC Pairing");
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let root = CertifiedIssuer::self_signed(root_params, rsa_key()?)?;
    let host = rsa_key()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::ExplicitNoCa;
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    let host_certificate = params.signed_by(&host, &root)?;
    let device_certificate = params.signed_by(&device, &root)?;
    let mut d = Dictionary::new();
    for (k, v) in [
        ("DeviceCertificate", device_certificate.pem()),
        ("HostCertificate", host_certificate.pem()),
        ("RootCertificate", root.pem()),
        ("HostPrivateKey", host.serialize_pem()),
    ] {
        // rcgen emits CRLF PEM on Windows. iOS can accept Pair with that text
        // yet terminate the subsequent TLS handshake. Store/send canonical LF.
        d.insert(k.into(), Value::Data(v.replace("\r\n", "\n").into_bytes()));
    }
    d.insert(
        "HostID".into(),
        Value::String(uuid::Uuid::new_v4().to_string().to_uppercase()),
    );
    d.insert("SystemBUID".into(), Value::String(buid.into()));
    d.insert("PairingFormatVersion".into(), Value::Integer(3.into()));
    Ok(PairRecord(d))
}

pub struct PairStore {
    directory: PathBuf,
}
impl PairStore {
    #[cfg(test)]
    pub fn at(directory: PathBuf) -> Self {
        Self { directory }
    }
    pub fn current_user() -> Result<Self> {
        let base = std::env::var_os("LOCALAPPDATA").context("Windows LOCALAPPDATA is missing")?;
        Ok(Self {
            directory: PathBuf::from(base).join("AirCard").join("pairing-v2"),
        })
    }
    fn path(&self, udid: &str) -> Result<PathBuf> {
        Ok(self
            .directory
            .join(format!("{}.dpapi", canonical_udid(udid)?)))
    }
    pub fn load(&self, udid: &str) -> Result<Option<PairRecord>> {
        let path = self.path(udid)?;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(bytes.len() < 128 * 1024, "Pair record is too large");
        let plaintext = protect(&bytes, false)
            .context("Cannot decrypt app pairing record for this Windows account")?;
        Ok(Some(PairRecord(
            Value::from_reader(Cursor::new(plaintext))?
                .into_dictionary()
                .context("Invalid app pairing record")?,
        )))
    }
    pub fn save(&self, udid: &str, record: &PairRecord) -> Result<()> {
        let mut plaintext = Vec::new();
        Value::Dictionary(record.0.clone()).to_writer_binary(&mut plaintext)?;
        let encrypted = protect(&plaintext, true)?;
        std::fs::create_dir_all(&self.directory)?;
        let target = self.path(udid)?;
        let temp = self.directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&encrypted)?;
            file.sync_all()?;
            drop(file);
            replace_file(&temp, &target)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
    pub fn buid(&self) -> Result<String> {
        std::fs::create_dir_all(&self.directory)?;
        let path = self.directory.join("host-buid.txt");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if uuid::Uuid::parse_str(s.trim()).is_ok() {
                return Ok(s.trim().into());
            }
        }
        let s = uuid::Uuid::new_v4().to_string().to_uppercase();
        std::fs::write(path, &s)?;
        Ok(s)
    }
}
#[cfg(windows)]
fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            desc: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            desc: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(p: *mut c_void) -> *mut c_void;
    }
    let input = Blob {
        len: bytes.len().try_into()?,
        data: bytes.as_ptr() as *mut u8,
    };
    let mut out = Blob {
        len: 0,
        data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut out,
            )
        }
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = unsafe { std::slice::from_raw_parts(out.data, out.len as usize).to_vec() };
    unsafe {
        LocalFree(out.data.cast());
    }
    Ok(result)
}
#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("Pairing storage requires Windows DPAPI")
}
#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1::EncodeRsaPublicKey;
    #[test]
    fn wrong_phone_certificate_is_rejected_and_private_key_not_sent() {
        let device = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let public = device
            .to_public_key()
            .to_pkcs1_pem(Default::default())
            .unwrap();
        let record = generate(public.as_bytes(), "test-buid").unwrap();
        assert!(record.matches_device(public.as_bytes()).unwrap());
        for key in [
            "RootCertificate",
            "HostCertificate",
            "DeviceCertificate",
            "HostPrivateKey",
        ] {
            assert!(
                !record.data(key).unwrap().contains(&b'\r'),
                "{key} must use LF PEM"
            );
        }
        let mut legacy = PairRecord(record.0.clone());
        legacy.0.remove("PairingFormatVersion");
        assert!(!legacy.matches_device(public.as_bytes()).unwrap());
        let other = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)
            .unwrap()
            .to_public_key()
            .to_pkcs1_pem(Default::default())
            .unwrap();
        assert!(!record.matches_device(other.as_bytes()).unwrap());
        let sent = record.public_record().unwrap();
        assert!(
            sent.as_dictionary()
                .unwrap()
                .get("HostPrivateKey")
                .is_none()
        );
        record.native_tls_connector().unwrap();
        let cert = certificate_der(record.data("DeviceCertificate").unwrap()).unwrap();
        record.verify_peer_certificate(cert.as_ref()).unwrap();
        assert!(
            record
                .verify_peer_certificate(b"wrong certificate")
                .is_err()
        );
    }
    #[test]
    #[cfg(windows)]
    fn encrypted_storage_roundtrip_and_replace() {
        let dir = std::env::temp_dir().join(format!("aircard-test-{}", uuid::Uuid::new_v4()));
        let store = PairStore {
            directory: dir.clone(),
        };
        let id = "00008130-0000000000000001";
        let mut d = Dictionary::new();
        d.insert(
            "HostPrivateKey".into(),
            Value::Data(b"secret-test-material".to_vec()),
        );
        let record = PairRecord(d);
        store.save(id, &record).unwrap();
        store.save(id, &record).unwrap();
        assert_eq!(
            store
                .load(id)
                .unwrap()
                .unwrap()
                .data("HostPrivateKey")
                .unwrap(),
            b"secret-test-material"
        );
        assert!(
            !std::fs::read(store.path(id).unwrap())
                .unwrap()
                .windows(20)
                .any(|w| w == b"secret-test-material")
        );
        assert!(store.path("../bad").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
