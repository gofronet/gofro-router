use std::{fs, net::Ipv4Addr, path::Path, process::Command};

use anyhow::{Context, Result, bail, ensure};

const USER_AGENT: &str = concat!("gofro-updater/", env!("CARGO_PKG_VERSION"));

fn checked(command: &mut Command, description: &str) -> Result<std::process::Output> {
    let output = command
        .output()
        .with_context(|| format!("failed to execute {description}"))?;
    if !output.status.success() {
        bail!(
            "{description} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

pub(crate) fn self_check() -> Result<()> {
    for (program, arguments) in [
        ("curl", &["--version"][..]),
        ("openssl", &["version"][..]),
        ("sha256sum", &["--version"][..]),
        ("tar", &["--version"][..]),
        ("systemctl", &["--version"][..]),
    ] {
        checked(
            Command::new(program).args(arguments),
            &format!("required command {program}"),
        )?;
    }
    let pkeyutl = checked(
        Command::new("openssl").args(["pkeyutl", "-help"]),
        "OpenSSL pkeyutl support",
    )?;
    ensure!(
        String::from_utf8_lossy(&pkeyutl.stdout).contains("-rawin")
            || String::from_utf8_lossy(&pkeyutl.stderr).contains("-rawin"),
        "OpenSSL pkeyutl does not support -rawin"
    );
    Ok(())
}

pub(crate) fn curl_download(url: &str, destination: &Path, maximum: u64) -> Result<()> {
    let maximum = maximum.to_string();
    checked(
        Command::new("curl")
            .args([
                "--retry",
                "3",
                "--retry-delay",
                "1",
                "--retry-max-time",
                "120",
                "--connect-timeout",
                "10",
                "--max-time",
                "120",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--remove-on-error",
                "--max-filesize",
                &maximum,
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--user-agent",
                USER_AGENT,
                "--output",
            ])
            .arg(destination)
            .arg("--")
            .arg(url),
        &format!("download from {url}"),
    )?;
    Ok(())
}

pub(crate) fn status_api() -> Result<Vec<u8>> {
    let address = fs::read_to_string(crate::paths::STATUS_ADDRESS)
        .context("failed to read local status address")?;
    let address: Ipv4Addr = address
        .trim()
        .parse()
        .context("local status address is not valid IPv4")?;
    let url = format!("http://{address}/api/status");
    Ok(checked(
        Command::new("curl")
            .args([
                "--connect-timeout",
                "2",
                "--max-time",
                "5",
                "--fail",
                "--silent",
                "--show-error",
                "--user-agent",
                USER_AGENT,
                "--",
            ])
            .arg(&url),
        "status API request",
    )?
    .stdout)
}

pub(crate) fn verify_signature(manifest: &Path, signature: &Path) -> Result<()> {
    checked(
        Command::new("openssl")
            .args(["pkeyutl", "-verify", "-pubin", "-inkey"])
            .arg(crate::paths::PUBLIC_KEY)
            .arg("-rawin")
            .arg("-in")
            .arg(manifest)
            .arg("-sigfile")
            .arg(signature),
        "manifest Ed25519 signature verification",
    )?;
    Ok(())
}

pub(crate) fn verify_sha256(archive: &Path, digest: &str) -> Result<()> {
    let output = checked(
        Command::new("sha256sum").arg("--").arg(archive),
        "archive SHA-256 verification",
    )?;
    let archive = archive
        .to_str()
        .context("archive path is not valid UTF-8")?;
    let expected = format!("{digest}  {archive}\n");
    ensure!(
        output.stdout == expected.as_bytes(),
        "archive SHA-256 output did not exactly match the signed digest"
    );
    Ok(())
}

pub(crate) fn extract(archive: &Path, destination: &Path) -> Result<()> {
    checked(
        Command::new("tar")
            .args([
                "--extract",
                "--gzip",
                "--no-same-owner",
                "--no-same-permissions",
                "--file",
            ])
            .arg(archive)
            .arg("--directory")
            .arg(destination),
        "safe archive extraction",
    )?;
    Ok(())
}

pub(crate) fn binary_check(binary: &Path, argument: &str, version: &str) -> Result<()> {
    let output = checked(
        Command::new(binary).arg(argument),
        &format!("{} {argument}", binary.display()),
    )?;
    let stdout = String::from_utf8(output.stdout)
        .with_context(|| format!("{} output was not UTF-8", binary.display()))?;
    let name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .context("bundle binary name is not valid UTF-8")?;
    ensure!(
        stdout == format!("{name} {version}\n"),
        "{} did not report version {version}",
        binary.display()
    );
    Ok(())
}

pub(crate) fn migration(release: &Path, action: &str, old: &str, new: &str) -> Result<()> {
    checked(
        Command::new(release.join("migrate.sh"))
            .args([action, old, new])
            .current_dir(release),
        &format!("migration {action} {old} {new}"),
    )?;
    Ok(())
}

pub(crate) fn systemctl(action: &str, service: &str) -> Result<()> {
    checked(
        Command::new("systemctl").args([action, service]),
        &format!("systemctl {action} {service}"),
    )?;
    Ok(())
}

pub(crate) fn service_active(service: &str) -> Result<bool> {
    Ok(Command::new("systemctl")
        .args(["is-active", "--quiet", service])
        .status()
        .with_context(|| format!("failed to query service {service}"))?
        .success())
}
