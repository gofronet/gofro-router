use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::Ipv4Addr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use wireguard_status::managed::FRIEND_NAME_INPUT_LIMIT;

use super::{
    Probe, UPGRADE_REQUIRED,
    pins::{management_directory, parse_host_key},
    temporary_path,
};
use crate::model::BootstrapStage;

const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SSH_UPDATE_TIMEOUT: Duration = Duration::from_secs(960);
const SSH_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(1200);
const SSH_OUTPUT_LIMIT: usize = 1024 * 1024;

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

pub(super) fn validate_target(host: &str, port: u16) -> Result<()> {
    let address = host.parse::<Ipv4Addr>().context(
        "Введите публичный IPv4-адрес VPS. IPv6 не поддерживается; получите IPv4 у провайдера VPS.",
    )?;
    if port == 0 {
        bail!("SSH-порт должен быть от 1 до 65535");
    }
    if host != address.to_string() || !is_public(address) {
        bail!("Введите публичный IPv4-адрес VPS из панели провайдера, без ведущих нулей.");
    }
    Ok(())
}

fn is_public(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
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

pub(super) fn management_key(dir: &Path) -> Result<PathBuf> {
    management_directory(dir)?;
    let key = dir.join("id_ed25519");
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

pub(super) fn known_hosts(dir: &Path, host: &str, port: u16, host_key: &str) -> Result<PathBuf> {
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

pub(super) fn password_ssh(
    host: &str,
    port: u16,
    known_hosts: &Path,
    password: &str,
    remote: &str,
    progress: &mut dyn FnMut(BootstrapStage),
) -> Result<()> {
    let mut command = Command::new("sshpass");
    command.args([
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
    ]);
    run_password_ssh(command, password, progress, SSH_BOOTSTRAP_TIMEOUT)
}

pub(super) fn run_password_ssh(
    mut command: Command,
    password: &str,
    progress: &mut dyn FnMut(BootstrapStage),
    timeout: Duration,
) -> Result<()> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());
    let mut child = command.spawn().context("failed to run sshpass")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to read sshpass stderr")?;
    let receiver = capture_diagnostics(stderr);
    let stdout = child.stdout.take().context("failed to read SSH progress")?;
    let (stages, stage_receiver) = mpsc::channel();
    thread::spawn(move || read_bootstrap_stages(stdout, stages));
    let input = child
        .stdin
        .take()
        .context("failed to open sshpass stdin")?
        .write_all(format!("{password}\n").as_bytes());
    if let Err(error) = input {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("failed to send SSH password");
    }
    let deadline = Instant::now() + timeout;
    while child.try_wait()?.is_none() {
        for stage in stage_receiver.try_iter() {
            progress(stage);
        }
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
    // The progress reader sends at most three events and drains all other output.
    while let Ok(stage) = stage_receiver.recv_timeout(Duration::from_secs(1)) {
        progress(stage);
    }
    let stderr = receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    bootstrap_result(status, &String::from_utf8_lossy(&stderr))
}

fn capture_diagnostics(mut stderr: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stderr.by_ref().take(8192).read_to_end(&mut output);
        let _ = sender.send(output);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    });
    receiver
}

fn read_bootstrap_stages(mut stdout: impl Read, sender: mpsc::Sender<BootstrapStage>) {
    let mut buffer = [0; 1024];
    let mut line = Vec::new();
    let mut oversized = false;
    let mut previous = 0;
    while let Ok(count) = stdout.read(&mut buffer) {
        if count == 0 {
            break;
        }
        for &byte in &buffer[..count] {
            if byte == b'\n' {
                let marker = match line.as_slice() {
                    b"GOFRO_STAGE inspect" => Some((1, BootstrapStage::Inspect)),
                    b"GOFRO_STAGE install" => Some((2, BootstrapStage::Install)),
                    b"GOFRO_STAGE authorize" => Some((3, BootstrapStage::Authorize)),
                    _ => None,
                };
                if !oversized
                    && let Some((order, stage)) = marker
                    && order > previous
                {
                    previous = order;
                    let _ = sender.send(stage);
                }
                line.clear();
                oversized = false;
            } else if line.len() < 64 {
                line.push(byte);
            } else {
                oversized = true;
            }
        }
    }
}

pub(super) fn validate_password(password: &str) -> Result<()> {
    if password.is_empty() || password.len() > 1024 || password.contains(['\r', '\n', '\0']) {
        bail!("Введите пароль root заново: он не должен быть пустым или содержать переносы строк.");
    }
    Ok(())
}

fn bootstrap_result(status: ExitStatus, stderr: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    if status.code() == Some(40) {
        bail!(UPGRADE_REQUIRED);
    }
    if status.code() == Some(41) {
        bail!(
            "Gofro is installed but the VPS is not ready: check its saved WireGuard configuration and VPN services. Existing peers were retained; no reinstall was attempted for a compatible server."
        );
    }
    // Classify diagnostics, but never expose raw SSH/installer output or credentials.
    if status.code() == Some(5) || stderr.contains("Permission denied (") {
        bail!(
            "VPS не разрешил вход root по паролю. Проверьте актуальный пароль после переустановки и настройки SSH."
        );
    }
    if matches!(status.code(), Some(6 | 7)) || stderr.contains("Host key verification failed") {
        bail!(
            "SSH-ключ VPS не совпал. Подключение отклонено; сохранённый ключ не изменён. Проверьте VPS через доверенную консоль."
        );
    }
    if status.code() == Some(22) && stderr.contains("404") {
        bail!(
            "Серверный пакет Gofro недоступен (HTTP 404). Для установки или обновления нужен опубликованный совместимый подписанный релиз."
        );
    }
    let installer_reason = if stderr.contains("release signature is invalid") {
        Some("Signed VPS release verification failed. Installation was refused.")
    } else if stderr.contains("release checksum does not match")
        || stderr.contains("release manifest is invalid")
        || stderr.contains("release bundle is invalid")
    {
        Some("VPS release integrity verification failed. Installation was refused.")
    } else if stderr.contains("update public key is missing") {
        Some(
            "VPS signed-update verification key is missing. Repair it through a trusted console; unsigned installation is not allowed.",
        )
    } else if stderr.contains("Debian or Ubuntu is required")
        || stderr.contains("x86_64 is required")
    {
        Some("VPS installation requires x86_64 Debian or Ubuntu.")
    } else if stderr.contains("another install or update is running") {
        Some("Another VPS installation or update is running. Check its outcome before retrying.")
    } else {
        None
    };
    if let Some(reason) = installer_reason {
        bail!("{reason} ({status})");
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

pub(super) fn key_ssh(
    host: &str,
    port: u16,
    private_key: &Path,
    dir: &Path,
    host_key: &str,
    remote: &str,
    input: Option<&str>,
) -> Result<String> {
    key_ssh_with_command(SshInvocation {
        host,
        port,
        private_key,
        dir,
        host_key,
        remote,
        input,
        executable: Path::new("ssh"),
        timeout: if remote == "update" {
            SSH_UPDATE_TIMEOUT
        } else {
            SSH_COMMAND_TIMEOUT
        },
    })
}

struct SshInvocation<'a> {
    host: &'a str,
    port: u16,
    private_key: &'a Path,
    dir: &'a Path,
    host_key: &'a str,
    remote: &'a str,
    input: Option<&'a str>,
    executable: &'a Path,
    timeout: Duration,
}

fn key_ssh_with_command(command: SshInvocation<'_>) -> Result<String> {
    if command
        .input
        .is_some_and(|value| value.len() > FRIEND_NAME_INPUT_LIMIT)
    {
        bail!("managed server command input is too large");
    }
    let known_hosts = known_hosts(command.dir, command.host, command.port, command.host_key)?;
    let child = Command::new(command.executable)
        .args([
            "-i",
            command
                .private_key
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
            &command.port.to_string(),
            &format!("root@{}", command.host),
            command.remote,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(
            command
                .input
                .is_some()
                .then(Stdio::piped)
                .unwrap_or_else(Stdio::null),
        )
        .spawn()
        .context("failed to run SSH");
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&known_hosts);
            return Err(error);
        }
    };
    let diagnostics = capture_diagnostics(
        child
            .stderr
            .take()
            .context("failed to read SSH diagnostics")?,
    );
    let input_error = command.input.and_then(|input| {
        child
            .stdin
            .take()
            .and_then(|mut stdin| stdin.write_all(input.as_bytes()).err())
    });
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
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
    let deadline = Instant::now() + command.timeout;
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
                Ok(Err(error)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error.into());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(anyhow!("failed to read SSH output"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error.into());
            }
        };
        if let Some(status) = status {
            let value = match output.take() {
                Some(value) => value,
                None => match receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => break Err(error.into()),
                    Err(_) => {
                        break Err(anyhow!(
                            "managed server SSH output timed out; remote outcome is unknown"
                        ));
                    }
                },
            };
            if value.len() > SSH_OUTPUT_LIMIT {
                break Err(anyhow!("managed server SSH output is too large"));
            }
            if !status.success() {
                let stderr = diagnostics
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap_or_default();
                break Err(ssh_command_error(
                    status,
                    &String::from_utf8_lossy(&stderr),
                    command.remote,
                ));
            }
            if input_error.is_some() {
                break Err(anyhow!("managed server SSH command input failed"));
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

pub(super) fn ssh_command_error(status: ExitStatus, stderr: &str, remote: &str) -> anyhow::Error {
    let reason = if stderr.contains("Host key verification failed")
        || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    {
        "SSH host key changed. Connection refused; the saved pin was not replaced."
    } else if stderr.contains("Permission denied (") {
        "VPS rejected the management SSH key. Check root public-key access and its restricted authorized_keys entry."
    } else if status.code() == Some(126)
        || (remote == "capabilities" && matches!(status.code(), Some(1 | 2 | 127)))
    {
        "Этот VPS использует устаревшую версию Gofro. VPS upgrade required: install a compatible signed release with owner protocol 2."
    } else if stderr.contains("metadata") || stderr.contains("legacy router owner") {
        "VPS ownership/friend metadata is missing, unsafe, or ambiguous. Existing access was retained; repair metadata through a trusted server console."
    } else if stderr.contains("unsafe server lock") || stderr.contains("server lock changed") {
        "VPS management lock is unsafe. Repair its ownership and permissions through a trusted console."
    } else if stderr.contains("no tunnel IP addresses") {
        "VPS has no free tunnel addresses. Re-add creates a new router access and retains old access; remove obsolete access through a trusted console."
    } else if stderr.contains("already assigned to another peer") {
        "VPS tunnel address or legacy LAN subnet belongs to another peer. Upgrade to owner protocol 2; existing access was not replaced."
    } else if stderr.contains("Cannot find device") || stderr.contains("No such device") {
        "VPS WireGuard interface is unavailable. Check the VPN services on the server."
    } else if stderr.contains("wg-quick") {
        "VPS could not persist or restart its WireGuard configuration. Check server storage and VPN services; the remote outcome may be partial."
    } else if status.code() == Some(124) {
        "VPS command timed out. Its outcome is unknown; inspect the server before retrying."
    } else if status.code() == Some(255) {
        "SSH connection failed. Check network, SSH port, and management key; the remote outcome may be unknown."
    } else {
        "VPS command failed. Check its VPN services and storage through a trusted console; no remote diagnostics or credentials are disclosed."
    };
    let operation = remote
        .split_whitespace()
        .next()
        .filter(|name| {
            matches!(
                *name,
                "capabilities"
                    | "version"
                    | "update"
                    | "managed-status"
                    | "restart-vpn"
                    | "create-router-profile"
                    | "remove-router-peer"
                    | "create-profile"
                    | "create-friend"
                    | "rename-friend"
                    | "revoke-friend"
                    | "friend-profile"
            )
        })
        .unwrap_or("management");
    anyhow!("{operation}: {reason} ({status})")
}

pub(super) fn ssh_keygen(args: &[&str]) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::super::tests::{Fixture, KEY, fake_ssh, fake_ssh_dir};
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn password_ssh_reports_live_stages_and_never_forwards_logs() {
        let fixture = Fixture::new();
        let mut command = Command::new("bash");
        command.arg("-c").arg("read -r password; printf 'secret\\nGOFRO_STAGE inspect\\n'; while [ ! -f \"$FIXTURE/seen\" ]; do sleep 0.01; done; printf 'GOFRO_STAGE authorize\\nGOFRO_STAGE inspect\\nGOFRO_STAGE authorize\\n'")
            .env("FIXTURE", &fixture.root);
        let mut stages = Vec::new();
        run_password_ssh(
            command,
            "secret",
            &mut |stage| {
                stages.push(stage);
                fs::write(fixture.root.join("seen"), "").unwrap();
            },
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(stages, [BootstrapStage::Inspect, BootstrapStage::Authorize]);
    }

    #[test]
    fn key_ssh_classifies_bounded_drained_stderr_without_secrets() {
        let fixture = Fixture::new();
        let script = fake_ssh(
            &fixture.root,
            "metadata-error",
            "#!/bin/sh\nprintf 'Error: missing metadata cannot distinguish router owners secret-private-key\\n' >&2\ni=0; while [ $i -lt 10000 ]; do printf 'secret-private-key\\n' >&2; i=$((i+1)); done; exit 1\n",
        );
        let error = fake_key_ssh(&fixture.root, &script, None, Duration::from_secs(5))
            .unwrap_err()
            .to_string();
        assert!(error.contains("metadata"));
        assert!(error.contains("managed-status"));
        assert!(error.contains("exit status: 1"));
        assert!(!error.contains("secret-private-key"));
        for diagnostic in [
            "Host key verification failed secret",
            "Permission denied (publickey). secret",
            "no tunnel IP addresses are available secret",
        ] {
            let error = ssh_command_error(
                ExitStatus::from_raw(255 << 8),
                diagnostic,
                "create-router-profile 1.1.1.1:8443",
            )
            .to_string();
            assert!(!error.contains("secret"));
            assert!(!error.contains("1.1.1.1"));
        }
    }
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
        assert!(validate_target("8.8.8.8", 65535).is_ok());
    }
    #[test]
    fn rejects_non_public_targets() {
        for host in [
            "2606:4700:4700::1111",
            "[2606:4700:4700::1111]",
            "::ffff:1.1.1.1",
            "2001:db8::1",
            "fe80::1%eth0",
            "vpn.example.com",
            "01.1.1.1",
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.1.1",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(
                validate_target(host, 22)
                    .unwrap_err()
                    .to_string()
                    .contains("IPv4"),
                "{host}"
            );
        }
        assert!(validate_target("1.1.1.1", 0).is_err());
    }

    #[test]
    fn ipv6_bootstrap_stops_before_ssh_or_local_mutation() {
        let fixture = Fixture::new();
        let before = fs::read(&fixture.state.config_path).unwrap();
        let mut stages = Vec::new();
        let error = super::super::bootstrap(
            &fixture.state,
            "VPS".into(),
            "2606:4700:4700::1111".into(),
            22,
            "password".into(),
            &mut |stage| stages.push(stage),
        )
        .unwrap_err();
        assert!(error.to_string().contains("IPv4"));
        assert_eq!(stages, [BootstrapStage::Waiting]);
        assert!(!fixture.state.management_dir.exists());
        assert_eq!(fs::read(&fixture.state.config_path).unwrap(), before);
    }

    #[test]
    fn ipv6_add_and_edit_leave_memory_and_disk_unchanged() {
        let fixture = Fixture::new();
        let mut server = fixture.server();
        server.management = None;
        crate::controller::add_server(&fixture.state, server.clone()).unwrap();
        let before = fs::read(&fixture.state.config_path).unwrap();
        let memory = serde_json::to_value(&*fixture.state.config.lock().unwrap()).unwrap();
        for endpoint in [
            "[2606:4700:4700::1111]:8443",
            "2606:4700:4700::1111:8443",
            "[::ffff:1.1.1.1]:8443",
            "[fe80::1%eth0]:8443",
        ] {
            let mut invalid = server.clone();
            invalid.endpoint = endpoint.into();
            assert!(
                crate::controller::import_server(&fixture.state, "Imported".into(), format!(
                    "[Interface]\nPrivateKey = {}\nAddress = 10.202.0.2/32\nMTU = 1280\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0\nEndpoint = {endpoint}\nPersistentKeepalive = 10\n",
                    server.client_private_key.as_deref().unwrap(), server.public_key,
                ))
                    .unwrap_err()
                    .to_string()
                    .contains("IPv4")
            );
            assert!(
                crate::controller::add_server(&fixture.state, invalid)
                    .unwrap_err()
                    .to_string()
                    .contains("IPv4")
            );
            let update = crate::model::ServerUpdate {
                previous_public_key: server.public_key.clone(),
                name: "Edited".into(),
                endpoint: endpoint.into(),
                public_key: server.public_key.clone(),
                emoji: None,
            };
            assert!(
                crate::controller::update_server(&fixture.state, update)
                    .unwrap_err()
                    .to_string()
                    .contains("IPv4")
            );
            assert_eq!(fs::read(&fixture.state.config_path).unwrap(), before);
            assert_eq!(
                serde_json::to_value(&*fixture.state.config.lock().unwrap()).unwrap(),
                memory
            );
        }
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

    fn fake_key_ssh(
        dir: &Path,
        executable: &Path,
        input: Option<&str>,
        timeout: Duration,
    ) -> Result<String> {
        key_ssh_with_command(SshInvocation {
            host: "1.1.1.1",
            port: 22,
            private_key: Path::new("unused-key"),
            dir,
            host_key: KEY,
            remote: "managed-status",
            input,
            executable,
            timeout,
        })
    }

    #[test]
    fn stdin_failures_still_reap_and_classify_old_vps() {
        let dir = fake_ssh_dir();
        fs::create_dir(&dir).unwrap();
        let script = fake_ssh(&dir, "exit-126", "#!/bin/sh\nexit 126\n");
        let error = fake_key_ssh(
            &dir,
            &script,
            Some(r#"{"name":"Friend"}"#),
            Duration::from_secs(30),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("устаревшую"), "{error}");
        assert!(fs::read_dir(&dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("known_hosts")
        }));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stdin_is_closed_and_commands_honor_the_test_timeout() {
        let dir = fake_ssh_dir();
        fs::create_dir(&dir).unwrap();
        let eof = fake_ssh(&dir, "wait-eof", "#!/bin/sh\ncat >/dev/null\n");
        assert_eq!(
            fake_key_ssh(
                &dir,
                &eof,
                Some(r#"{"name":"Friend"}"#),
                Duration::from_secs(30)
            )
            .unwrap(),
            ""
        );
        let busy = fake_ssh(&dir, "busy", "#!/bin/sh\nwhile :; do :; done\n");
        let error = fake_key_ssh(&dir, &busy, None, Duration::from_millis(1)).unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error}");
        assert!(fs::read_dir(&dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("known_hosts")
        }));
        fs::remove_dir_all(dir).unwrap();
    }
}
