use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::Ipv4Addr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, bail};
use openssl::{
    asn1::{Asn1Integer, Asn1Time},
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::PKey,
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
    },
};

pub(crate) fn ensure(cert: &Path, key: &Path, address: Ipv4Addr) -> Result<String> {
    let certificate_tmp = cert.with_extension("new");
    let key_tmp = key.with_extension("new");
    match (cert.exists(), key.exists()) {
        (false, false) => {
            let _ = fs::remove_file(&certificate_tmp);
            let _ = fs::remove_file(&key_tmp);
            generate(cert, key, address)?;
        }
        (true, true) => {
            validate(cert, key)?;
            if !has_address(cert, address)? {
                generate(cert, key, address)?;
            }
        }
        (false, true) if certificate_tmp.exists() => {
            validate(&certificate_tmp, key)?;
            fs::rename(certificate_tmp, cert)?;
            let _ = fs::remove_file(key_tmp);
        }
        (true, false) if key_tmp.exists() => {
            validate(cert, &key_tmp)?;
            fs::rename(key_tmp, key)?;
            let _ = fs::remove_file(certificate_tmp);
        }
        _ => bail!("TLS certificate and key must be created together"),
    }
    validate(cert, key)?;
    if !has_address(cert, address)? {
        generate(cert, key, address)?;
        validate(cert, key)?;
    }
    fingerprint(&fs::read(cert)?)
}

fn generate(cert: &Path, key: &Path, address: Ipv4Addr) -> Result<()> {
    if let Some(parent) = cert.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = key.parent() {
        fs::create_dir_all(parent)?;
    }
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
    let private = if key.exists() {
        PKey::private_key_from_pem(&fs::read(key)?)?
    } else {
        PKey::from_ec_key(EcKey::generate(&group)?)?
    };
    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_text("CN", "wifi.gofro.net")?;
    let name = name.build();
    let mut certificate = X509::builder()?;
    certificate.set_version(2)?;
    let mut serial = [0; 16];
    openssl::rand::rand_bytes(&mut serial)?;
    serial[0] &= 0x7f;
    serial[0] |= 1;
    let serial = BigNum::from_slice(&serial)?;
    let serial = Asn1Integer::from_bn(&serial)?;
    certificate.set_serial_number(&serial)?;
    certificate.set_subject_name(&name)?;
    certificate.set_issuer_name(&name)?;
    certificate.set_pubkey(&private)?;
    let not_before = Asn1Time::from_unix(0)?;
    let not_after = Asn1Time::from_str_x509("20500101000000Z")?;
    certificate.set_not_before(&not_before)?;
    certificate.set_not_after(&not_after)?;
    certificate.append_extension(BasicConstraints::new().critical().build()?)?;
    certificate.append_extension(KeyUsage::new().critical().digital_signature().build()?)?;
    certificate.append_extension(ExtendedKeyUsage::new().server_auth().build()?)?;
    let context = certificate.x509v3_context(None, None);
    certificate.append_extension(
        SubjectAlternativeName::new()
            .dns("wifi.gofro.net")
            .ip(&address.to_string())
            .build(&context)?,
    )?;
    certificate.sign(&private, MessageDigest::sha256())?;
    let certificate_tmp = cert.with_extension("new");
    let key_tmp = key.with_extension("new");
    write_private(&certificate_tmp, &certificate.build().to_pem()?)?;
    if !key.exists() {
        write_private(&key_tmp, &private.private_key_to_pem_pkcs8()?)?;
        fs::rename(key_tmp, key)?;
    }
    fs::rename(certificate_tmp, cert)?;
    Ok(())
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}
fn validate(cert: &Path, key: &Path) -> Result<()> {
    if fs::metadata(key)?.permissions().mode() & 0o077 != 0 {
        bail!("TLS private key permissions are unsafe");
    }
    let certificate = X509::from_pem(&fs::read(cert)?)?;
    let private = PKey::private_key_from_pem(&fs::read(key)?)?;
    if !certificate.public_key()?.public_eq(&private) {
        bail!("TLS certificate does not match private key");
    }
    if !certificate
        .subject_alt_names()
        .context("TLS certificate has no SAN")?
        .iter()
        .any(|san| san.dnsname() == Some("wifi.gofro.net"))
    {
        bail!("TLS certificate SAN is invalid");
    }
    Ok(())
}
fn has_address(cert: &Path, address: Ipv4Addr) -> Result<bool> {
    Ok(X509::from_pem(&fs::read(cert)?)?
        .subject_alt_names()
        .context("TLS certificate has no SAN")?
        .iter()
        .any(|san| san.ipaddress() == Some(address.octets().as_slice())))
}
fn fingerprint(pem: &[u8]) -> Result<String> {
    Ok(X509::from_pem(pem)?
        .digest(MessageDigest::sha256())?
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_certificate_has_stable_fingerprint_and_secure_key() {
        let dir = std::env::temp_dir().join(format!("gofro-tls-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        let first = ensure(&cert, &key, Ipv4Addr::new(192, 168, 0, 1)).unwrap();
        assert_eq!(
            X509::from_pem(&fs::read(&cert).unwrap())
                .unwrap()
                .not_after()
                .to_string(),
            "Jan  1 00:00:00 2050 GMT"
        );
        fs::rename(&cert, cert.with_extension("new")).unwrap();
        assert_eq!(
            first,
            ensure(&cert, &key, Ipv4Addr::new(192, 168, 0, 1)).unwrap()
        );
        assert_eq!(
            fs::metadata(key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reissues_for_a_changed_lan_address_with_the_existing_key() {
        let dir = std::env::temp_dir().join(format!("gofro-tls-address-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        ensure(&cert, &key, Ipv4Addr::new(192, 168, 0, 1)).unwrap();
        let before = fs::read(&key).unwrap();
        ensure(&cert, &key, Ipv4Addr::new(192, 168, 1, 1)).unwrap();
        assert_eq!(before, fs::read(&key).unwrap());
        assert!(has_address(&cert, Ipv4Addr::new(192, 168, 1, 1)).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
