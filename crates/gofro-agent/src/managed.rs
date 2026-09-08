use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::IpAddr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

use wireguard_status::managed::{
    FRIEND_NAME_INPUT_LIMIT, FriendNameInput, ManagedServerStatus, validate_name, validate_peer_key,
};

use crate::{
    AppState,
    config::{normalize_server_name, parse_server_profile},
    controller,
    model::{ControllerConfig, ManagedServer},
};

static TEMPORARY: AtomicUsize = AtomicUsize::new(0);
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
    mut name: String,
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
    normalize_server_name(&mut name)?;
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
        None,
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
            None,
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
        None,
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
        None,
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
        None,
    )
}

pub(crate) fn status(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    status_unlocked(state, public_key)
}

pub(crate) fn restart(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        "restart-vpn",
        None,
    )?;
    status_unlocked(state, public_key)
}

pub(crate) fn create_friend(
    state: &AppState,
    public_key: &str,
    name: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    let name = validate_name(name)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    let input = serde_json::to_string(&FriendNameInput { name })?;
    // The raw profile contains a private key and must not enter status or configuration.
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("create-friend {}", endpoint(&managed.host)),
        Some(&input),
    )?;
    status_unlocked(state, public_key)
}

pub(crate) fn rename_friend(
    state: &AppState,
    public_key: &str,
    peer_key: &str,
    name: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    let name = validate_name(name)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    let input = serde_json::to_string(&FriendNameInput { name })?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("rename-friend {peer_key}"),
        Some(&input),
    )?;
    status_unlocked(state, public_key)
}

pub(crate) fn revoke_friend(
    state: &AppState,
    public_key: &str,
    peer_key: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("revoke-friend {peer_key}"),
        None,
    )?;
    status_unlocked(state, public_key)
}

pub(crate) fn friend_profile(state: &AppState, public_key: &str, peer_key: &str) -> Result<String> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("friend-profile {peer_key} {}", endpoint(&managed.host)),
        None,
    )
}

fn managed_lock(state: &AppState) -> Result<std::sync::MutexGuard<'_, ()>> {
    state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))
}

fn status_unlocked(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let (managed, private_key) = managed_server(state, public_key)?;
    let output = key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        "managed-status",
        None,
    )?;
    let status: ManagedServerStatus =
        serde_json::from_str(&output).context("invalid managed server status")?;
    validate_managed_status(&status)?;
    Ok(status)
}

fn validate_managed_status(status: &ManagedServerStatus) -> Result<()> {
    semver(&status.version)?;
    let mut peers = HashSet::new();
    for peer in &status.peers {
        validate_peer_key(&peer.public_key)?;
        if !peers.insert(&peer.public_key) {
            bail!("managed server returned duplicate peer key");
        }
        if peer.can_share && peer.revoked {
            bail!("managed server returned an invalid friend state");
        }
        if validate_name(&peer.name)? != peer.name {
            bail!("managed server returned an unnormalized friend name");
        }
    }
    Ok(())
}

fn managed_server(state: &AppState, public_key: &str) -> Result<(ManagedServer, PathBuf)> {
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let management = managed_server_config(&config, public_key)?;
    drop(config);
    let private_key = state.management_dir.join("id_ed25519");
    if !private_key.is_file() {
        bail!("management SSH key is missing");
    }
    Ok((management, private_key))
}

fn managed_server_config(config: &ControllerConfig, public_key: &str) -> Result<ManagedServer> {
    validate_peer_key(public_key)?;
    config
        .servers
        .iter()
        .find(|server| server.public_key == public_key)
        .context("сервер не найден")?
        .management
        .clone()
        .context("сервер не управляется Gofro")
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
        .stderr(Stdio::null())
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
                None => receiver.recv().context("failed to read SSH output")??,
            };
            if value.len() > SSH_OUTPUT_LIMIT {
                break Err(anyhow!("managed server SSH output is too large"));
            }
            if !status.success() {
                break Err(ssh_command_error(status));
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

fn ssh_command_error(status: ExitStatus) -> anyhow::Error {
    if status.code() == Some(126) {
        anyhow!(
            "Этот VPS использует устаревшую версию Gofro. Обновите Gofro на VPS и повторите действие."
        )
    } else if status.code() == Some(255) {
        anyhow!(
            "Не удалось подключиться к VPS по SSH. Проверьте сеть, IP-адрес, порт и SSH-ключ; это не обязательно означает, что VPS выключен."
        )
    } else {
        anyhow!("managed server SSH command failed ({status})")
    }
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

    #[test]
    fn validates_typed_managed_status_and_upgrade_guidance() {
        let key = "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
        let status = ManagedServerStatus {
            version: "0.5.14".into(),
            peers: vec![wireguard_status::managed::FriendPeer {
                public_key: key.into(),
                name: "Friend".into(),
                revoked: false,
                can_share: true,
            }],
        };
        assert!(validate_managed_status(&status).is_ok());
        let mut invalid = status.clone();
        invalid.peers[0].revoked = true;
        assert!(validate_managed_status(&invalid).is_err());
        assert!(
            ssh_command_error(ExitStatus::from_raw(126 << 8))
                .to_string()
                .contains("устаревшую")
        );
    }

    #[test]
    fn unmanaged_servers_are_rejected_before_ssh() {
        let key = "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
        let config = ControllerConfig {
            vpn_enabled: false,
            active_server_key: None,
            servers: vec![crate::model::ServerProfile {
                name: "Unmanaged".into(),
                emoji: String::new(),
                endpoint: "vpn.example.com:8443".into(),
                public_key: key.into(),
                client_tunnel_address: None,
                client_private_key: None,
                management: None,
            }],
            routing: crate::model::RoutingConfig::default(),
        };
        assert!(managed_server_config(&config, key).is_err());
    }

    fn fake_ssh(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
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

    fn fake_ssh_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "gofro-ssh-{}-{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed),
        ))
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
            Duration::from_secs(1),
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
                Duration::from_secs(1)
            )
            .unwrap(),
            ""
        );
        let busy = fake_ssh(&dir, "busy", "#!/bin/sh\nwhile :; do :; done\n");
        let started = Instant::now();
        assert!(fake_key_ssh(&dir, &busy, None, Duration::from_millis(1)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
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
