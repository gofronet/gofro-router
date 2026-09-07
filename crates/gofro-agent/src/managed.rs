use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::IpAddr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{AppState, config::parse_server_profile, controller, model::ManagedServer};

static TEMPORARY: AtomicU64 = AtomicU64::new(0);
const SERVER_INSTALLER: &str = include_str!("../../../deploy/server/gofro-server-install");
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SSH_UPDATE_TIMEOUT: Duration = Duration::from_secs(960);
const SSH_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(1200);
const SSH_OUTPUT_LIMIT: usize = 1024 * 1024;

pub(crate) struct Probe {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) host_key: String,
    pub(crate) fingerprint: String,
}

pub(crate) struct Version {
    pub(crate) version: String,
    pub(crate) update_available: bool,
}

pub(crate) fn probe(host: String, port: u16) -> Result<Probe> {
    validate_target(&host, port)?;
    let output = Command::new("ssh-keyscan")
        .args(["-T", "10", "-t", "ed25519", "-p", &port.to_string(), &host])
        .output()
        .context("failed to run ssh-keyscan")?;
    let host_key = parse_keyscan(&String::from_utf8_lossy(&output.stdout))?;
    let fingerprint = fingerprint(&host_key)?;
    Ok(Probe {
        host,
        port,
        host_key,
        fingerprint,
    })
}

pub(crate) fn bootstrap(
    state: &AppState,
    name: String,
    host: String,
    port: u16,
    password: String,
    host_key: String,
) -> Result<()> {
    validate_password(&password)?;
    let _operation = state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))?;
    if name.trim().is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        bail!("некорректное имя сервера");
    }
    validate_target(&host, port)?;
    parse_host_key(&host_key)?;
    if state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?
        .servers
        .iter()
        .any(|server| {
            server
                .management
                .as_ref()
                .is_some_and(|managed| managed.host == host && managed.port == port)
        })
    {
        bail!("управляемый сервер с этим host уже существует");
    }
    let fresh = probe(host.clone(), port)?;
    if fresh.host_key != host_key {
        bail!("SSH host key changed since confirmation");
    }
    let private_key = management_key(state)?;
    let public_key = ssh_keygen(&[
        "-y",
        "-f",
        private_key
            .to_str()
            .context("invalid management key path")?,
    ])?;
    let known_hosts = known_hosts(&state.management_dir, &host, port, &host_key)?;
    let authorized = format!(
        "restrict,command=\"/usr/local/sbin/gofro-managed\" {}",
        public_key.trim()
    );
    let remote = format!(
        "set -eu; install -m 700 /dev/null /tmp/gofro-server-install; printf %s {} > /tmp/gofro-server-install; if [ -s /etc/gofro/version ] && [ \"$(cat /etc/gofro/version)\" = {} ]; then :; elif [ -s /etc/gofro/version ]; then bash /tmp/gofro-server-install; else bash /tmp/gofro-server-install --install; fi; rm -f /tmp/gofro-server-install; install -d -m 700 /root/.ssh; touch /root/.ssh/authorized_keys; chmod 600 /root/.ssh/authorized_keys; grep -qxF {} /root/.ssh/authorized_keys || printf '\\n%s\\n' {} >> /root/.ssh/authorized_keys",
        shell_quote(SERVER_INSTALLER),
        shell_quote(env!("CARGO_PKG_VERSION")),
        shell_quote(&authorized),
        shell_quote(&authorized)
    );
    let result = password_ssh(&host, port, &known_hosts, &password, &remote);
    let _ = fs::remove_file(&known_hosts);
    result?;

    let profile = key_ssh(
        &host,
        port,
        &private_key,
        &state.management_dir,
        &host_key,
        &format!("create-router-profile {}", endpoint(&host)),
    )?;
    let client_public_key = profile_client_public_key(&profile)?;
    let result = (|| {
        let mut server = parse_server_profile(name, &profile)?;
        server.management = Some(ManagedServer {
            host: host.clone(),
            port,
            host_key: host_key.clone(),
        });
        controller::add_server(state, server)
    })();
    if let Err(error) = result {
        let rollback = key_ssh(
            &host,
            port,
            &private_key,
            &state.management_dir,
            &host_key,
            &format!("remove-router-peer {client_public_key}"),
        );
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback) => Err(anyhow!(
                "local server enrollment failed: {error:#}; remote peer rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

pub(crate) fn check(state: &AppState, public_key: &str) -> Result<Version> {
    let _operation = state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))?;
    check_unlocked(state, public_key)
}
fn check_unlocked(state: &AppState, public_key: &str) -> Result<Version> {
    let (managed, private_key) = managed_server(state, public_key)?;
    let output = key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        "version",
    )?;
    let version = parse_version(&output)?;
    Ok(Version {
        update_available: semver(&version)? < semver(env!("CARGO_PKG_VERSION"))?,
        version,
    })
}

pub(crate) fn update(state: &AppState, public_key: &str) -> Result<Version> {
    let _operation = state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        "update",
    )?;
    check_unlocked(state, public_key)
}

pub(crate) fn create_profile(state: &AppState, public_key: &str) -> Result<String> {
    let _operation = state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("create-profile {}", endpoint(&managed.host)),
    )
}

fn managed_server(state: &AppState, public_key: &str) -> Result<(ManagedServer, PathBuf)> {
    let management = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?
        .servers
        .iter()
        .find(|server| server.public_key == public_key)
        .context("сервер не найден")?
        .management
        .clone()
        .context("сервер не управляется Gofro")?;
    let private_key = state.management_dir.join("id_ed25519");
    if !private_key.is_file() {
        bail!("management SSH key is missing");
    }
    Ok((management, private_key))
}

fn validate_target(host: &str, port: u16) -> Result<()> {
    if host.parse::<IpAddr>().is_err() || port == 0 {
        bail!("host must be a canonical IP address and port must be nonzero");
    }
    let address = host.parse::<IpAddr>()?;
    if host != address.to_string() || !is_public(address) {
        bail!("host must be a canonical public IP address");
    }
    Ok(())
}

fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [a, b, c, _] = address.octets();
            !matches!(a, 0 | 10 | 127 | 224..=255)
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
                && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && segments[1] <= 0x01ff)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

pub(crate) fn parse_host_key(value: &str) -> Result<()> {
    let mut parts = value.split_whitespace();
    let (Some(kind), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
        bail!("invalid ed25519 host key");
    };
    if kind != "ssh-ed25519"
        || key.len() != 68
        || !key.starts_with("AAAAC3NzaC1lZDI1NTE5AAAAI")
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        bail!("invalid ed25519 host key");
    }
    Ok(())
}

fn parse_keyscan(value: &str) -> Result<String> {
    let keys = value
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _host = fields.next();
            let kind = fields.next().context("invalid ssh-keyscan output")?;
            let key = fields.next().context("invalid ssh-keyscan output")?;
            if fields.next().is_some() {
                bail!("invalid ssh-keyscan output");
            }
            let value = format!("{kind} {key}");
            parse_host_key(&value)?;
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    match keys.as_slice() {
        [key] => Ok(key.clone()),
        _ => bail!("expected exactly one ed25519 SSH host key"),
    }
}

fn fingerprint(host_key: &str) -> Result<String> {
    let mut child = Command::new("ssh-keygen")
        .args(["-lf", "-", "-E", "sha256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to run ssh-keygen")?;
    child
        .stdin
        .take()
        .context("failed to open ssh-keygen stdin")?
        .write_all(format!("{host_key}\n").as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("ssh-keygen rejected SSH host key");
    }
    String::from_utf8(output.stdout).context("invalid ssh-keygen output")
}

pub(crate) fn parse_version(value: &str) -> Result<String> {
    let value = value.trim();
    let Some(version) = value.strip_prefix("gofro-server ") else {
        bail!("unexpected server version output");
    };
    if version.is_empty()
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        bail!("unexpected server version output");
    }
    Ok(version.to_owned())
}

fn management_key(state: &AppState) -> Result<PathBuf> {
    fs::create_dir_all(&state.management_dir)?;
    fs::set_permissions(&state.management_dir, fs::Permissions::from_mode(0o700))?;
    let key = state.management_dir.join("id_ed25519");
    if !key.exists() {
        run(Command::new("ssh-keygen").args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            key.to_str().context("invalid management directory")?,
        ]))?;
    }
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
    Ok(key)
}

fn known_hosts(dir: &Path, host: &str, port: u16, host_key: &str) -> Result<PathBuf> {
    let path = temporary_path(dir, "known_hosts");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    let target = if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    };
    file.write_all(format!("{target} {host_key}\n").as_bytes())?;
    Ok(path)
}

fn temporary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(
        ".{name}.{}.{}",
        std::process::id(),
        TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ))
}

fn password_ssh(
    host: &str,
    port: u16,
    known_hosts: &Path,
    password: &str,
    remote: &str,
) -> Result<()> {
    let mut command = Command::new("sshpass");
    command
        .args([
            "-d",
            "0",
            "ssh",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            &format!("UserKnownHostsFile={}", known_hosts.display()),
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "PasswordAuthentication=yes",
            "-o",
            "KbdInteractiveAuthentication=yes",
            "-o",
            "PreferredAuthentications=password,keyboard-interactive",
            "-o",
            "NumberOfPasswordPrompts=1",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-p",
            &port.to_string(),
            &format!("root@{host}"),
            remote,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());
    let mut child = command.spawn().context("failed to run sshpass")?;
    let mut stderr = child
        .stderr
        .take()
        .context("failed to read sshpass stderr")?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stderr.by_ref().take(8192).read_to_end(&mut output);
        let _ = sender.send(output);
        // Drain the rest without retaining remote-controlled output in memory.
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    });
    child
        .stdin
        .take()
        .context("failed to open sshpass stdin")?
        .write_all(format!("{password}\n").as_bytes())?;
    let deadline = Instant::now() + SSH_BOOTSTRAP_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "Истекло время настройки VPS. Проверьте состояние сервера перед повторной попыткой."
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
    let status = child.wait()?;
    let stderr = receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    bootstrap_result(status, &String::from_utf8_lossy(&stderr))
}

fn validate_password(password: &str) -> Result<()> {
    if password.is_empty() || password.len() > 1024 || password.contains(['\r', '\n', '\0']) {
        bail!("Введите пароль root заново: он не должен быть пустым или содержать переносы строк.");
    }
    Ok(())
}

fn bootstrap_result(status: ExitStatus, stderr: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    // Classify diagnostics, but never expose raw SSH/installer output or credentials.
    if status.code() == Some(5) || stderr.contains("Permission denied (") {
        bail!(
            "VPS не разрешил вход root по паролю. Проверьте актуальный пароль после переустановки и настройки SSH."
        );
    }
    if matches!(status.code(), Some(6 | 7)) || stderr.contains("Host key verification failed") {
        bail!(
            "SSH-ключ VPS не совпал. Проверьте отпечаток после переустановки и выполните проверку VPS заново."
        );
    }
    if status.code() == Some(22) && stderr.contains("404") {
        bail!(
            "Серверный пакет Gofro ещё не опубликован (HTTP 404). Установка с нуля станет доступна после публикации релиза."
        );
    }
    if status.code() == Some(255) {
        bail!(
            "Не удалось подключиться к VPS по SSH. Проверьте IP-адрес, порт и доступность сервера."
        );
    }
    bail!(
        "Установка Gofro на VPS завершилась с ошибкой ({status}). Проверьте ОС сервера и доступ к пакетам и релизам."
    )
}

fn key_ssh(
    host: &str,
    port: u16,
    private_key: &Path,
    dir: &Path,
    host_key: &str,
    remote: &str,
) -> Result<String> {
    let known_hosts = known_hosts(dir, host, port, host_key)?;
    let child = Command::new("ssh")
        .args([
            "-i",
            private_key
                .to_str()
                .context("invalid management key path")?,
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("UserKnownHostsFile={}", known_hosts.display()),
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-p",
            &port.to_string(),
            &format!("root@{host}"),
            remote,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to run SSH");
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&known_hosts);
            return Err(error);
        }
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = fs::remove_file(&known_hosts);
        bail!("failed to read SSH output");
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout
            .take((SSH_OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output);
        let _ = sender.send(result);
    });
    let deadline = Instant::now()
        + if remote == "update" {
            SSH_UPDATE_TIMEOUT
        } else {
            SSH_COMMAND_TIMEOUT
        };
    let mut output = None;
    let result = loop {
        if output.is_none() {
            match receiver.try_recv() {
                Ok(Ok(value)) if value.len() > SSH_OUTPUT_LIMIT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(anyhow!("managed server SSH output is too large"));
                }
                Ok(Ok(value)) => output = Some(value),
                Ok(Err(error)) => break Err(error.into()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    break Err(anyhow!("failed to read SSH output"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(status) = child.try_wait()? {
            let value = match output.take() {
                Some(value) => value,
                None => receiver.recv().context("failed to read SSH output")??,
            };
            if value.len() > SSH_OUTPUT_LIMIT {
                break Err(anyhow!("managed server SSH output is too large"));
            }
            if !status.success() {
                break Err(anyhow!("managed server SSH command failed"));
            }
            break String::from_utf8(value).context("managed server returned invalid text");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Err(anyhow!("managed server SSH command timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let _ = fs::remove_file(&known_hosts);
    result
}

fn ssh_keygen(args: &[&str]) -> Result<String> {
    let output = Command::new("ssh-keygen").args(args).output()?;
    if !output.status.success() {
        bail!("ssh-keygen failed");
    }
    String::from_utf8(output.stdout).context("invalid ssh-keygen output")
}
fn run(command: &mut Command) -> Result<()> {
    if !command.status()?.success() {
        bail!("command failed");
    }
    Ok(())
}

fn endpoint(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:8443"),
        _ => format!("{host}:8443"),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn profile_client_public_key(profile: &str) -> Result<&str> {
    let key = profile
        .lines()
        .find_map(|line| line.strip_prefix("# ClientPublicKey = "))
        .context("server profile has no client public key")?;
    let bytes = key.as_bytes();
    if bytes.len() != 44
        || bytes[43] != b'='
        || !bytes[..43]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        bail!("server profile has an invalid client public key");
    }
    Ok(key)
}

fn semver(value: &str) -> Result<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let version = (
        parts.next().context("invalid version")?.parse()?,
        parts.next().context("invalid version")?.parse()?,
        parts.next().context("invalid version")?.parse()?,
    );
    if parts.next().is_some() {
        bail!("invalid version");
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEwCks2omLrfMrS1du13ol2Iwo4CoDhME50jxaLI7Mdq";
    #[test]
    fn parses_one_host_key() {
        assert_eq!(parse_keyscan(&format!("[::1]:22 {KEY}\n")).unwrap(), KEY);
    }
    #[test]
    fn rejects_multiple_host_keys() {
        assert!(parse_keyscan(&format!("a {KEY}\nb {KEY}\n")).is_err());
    }
    #[test]
    fn accepts_public_targets() {
        assert!(validate_target("1.1.1.1", 22).is_ok());
        assert!(validate_target("2606:4700:4700::1111", 22).is_ok());
    }
    #[test]
    fn rejects_non_public_targets() {
        assert!(validate_target("192.168.1.1", 22).is_err());
        assert!(validate_target("2001:db8::1", 22).is_err());
    }
    #[test]
    fn parses_exact_version() {
        assert_eq!(parse_version("gofro-server 0.5.11\n").unwrap(), "0.5.11");
        assert!(parse_version("0.5.11").is_err());
    }

    #[test]
    fn quotes_shell_data_without_leaving_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn validates_password_and_reports_bootstrap_failure_without_remote_output() {
        assert!(validate_password("ke2?#test-password").is_ok());
        for password in ["", "password\nextra", "password\r", "password\0"] {
            assert!(validate_password(password).is_err());
        }
        for (code, stderr, expected) in [
            (5, "", "паролю"),
            (
                255,
                "root@example: Permission denied (publickey,password). secret",
                "паролю",
            ),
            (255, "Host key verification failed. secret", "SSH-ключ"),
            (
                22,
                "curl: (22) The requested URL returned error: 404 secret",
                "HTTP 404",
            ),
            (100, "secret", "ошибкой"),
        ] {
            let error = bootstrap_result(ExitStatus::from_raw(code << 8), stderr)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("secret"));
        }
        assert!(bootstrap_result(ExitStatus::from_raw(0), "").is_ok());
    }
}
