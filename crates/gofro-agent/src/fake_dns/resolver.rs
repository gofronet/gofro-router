use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    path::Path,
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{RData, Record, RecordType, rdata::A},
};
use socket2::{Domain, Protocol, Socket, Type};

use crate::{
    config::normalize_domain,
    dataplane::{self, FakeMapping},
    model::{AP_DOMAIN, LanContext, PANEL_VIRTUAL_IP, RouteTarget},
    routing::{RoutingPolicy, is_lan_destination},
};

use super::store::Store;

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
pub(super) const MAX_DNS_PACKET: usize = u16::MAX as usize;

pub(crate) struct FakeDns {
    store: Mutex<Store>,
    pub(super) updates: RwLock<()>,
    pub(super) active: AtomicBool,
    vpn_enabled: AtomicBool,
}

impl FakeDns {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        Ok(Self {
            store: Mutex::new(Store::open(path)?),
            updates: RwLock::new(()),
            active: AtomicBool::new(false),
            vpn_enabled: AtomicBool::new(false),
        })
    }

    pub(crate) fn reclassified(&self, policy: &RoutingPolicy) -> Result<Vec<FakeMapping>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow!("FakeDNS store lock poisoned"))?;
        Ok(store.reclassified(policy))
    }

    pub(crate) fn commit_targets(&self, policy: &RoutingPolicy) -> Result<()> {
        self.store
            .lock()
            .map_err(|_| anyhow!("FakeDNS store lock poisoned"))?
            .commit_targets(policy)
    }

    pub(crate) fn count(&self) -> usize {
        self.store.lock().map_or(0, |store| store.len())
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub(crate) fn set_vpn_enabled(&self, enabled: bool) {
        self.vpn_enabled.store(enabled, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn vpn_enabled(&self) -> bool {
        self.vpn_enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn begin_update(&self) -> Result<std::sync::RwLockWriteGuard<'_, ()>> {
        self.updates
            .write()
            .map_err(|_| anyhow!("FakeDNS update lock poisoned"))
    }

    pub(super) fn purge_expired(&self) -> Result<usize> {
        let _update = self
            .updates
            .read()
            .map_err(|_| anyhow!("FakeDNS update lock poisoned"))?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| anyhow!("FakeDNS store lock poisoned"))?;
        let expired = store.expired()?;
        if expired.is_empty() {
            return Ok(0);
        }
        dataplane::remove_mappings(&expired)?;
        if let Err(error) = store.remove_mappings(&expired) {
            return match dataplane::install_mappings(&expired) {
                Ok(()) => Err(error),
                Err(rollback) => Err(anyhow!(
                    "expired lease cleanup failed: {error:#}; dataplane rollback failed: {rollback:#}"
                )),
            };
        }
        Ok(expired.len())
    }

    pub(super) fn process(
        &self,
        packet: &[u8],
        policy: &RoutingPolicy,
        upstream: SocketAddr,
        lan: &LanContext,
    ) -> Result<Vec<u8>> {
        let request = Message::from_vec(packet).context("invalid DNS request")?;
        let query = request.query().context("DNS request has no question")?;
        let domain = query_domain(&query.name().to_utf8());
        if domain == AP_DOMAIN {
            return panel_response(&request);
        }
        let response = query_upstream(packet, upstream)?;
        let mut message = Message::from_vec(&response).context("invalid upstream DNS response")?;
        if message.message_type() != MessageType::Response
            || message.id() != request.id()
            || message.queries() != request.queries()
        {
            bail!("upstream DNS response does not match the request");
        }
        self.rewrite_response(&mut message, &domain, policy, lan)?;
        message.to_vec().context("failed to encode DNS response")
    }

    fn rewrite_response(
        &self,
        message: &mut Message,
        domain: &str,
        policy: &RoutingPolicy,
        lan: &LanContext,
    ) -> Result<()> {
        let vpn_enabled = self.vpn_enabled.load(Ordering::Relaxed);
        let domain_target = policy.domain_target(domain).map(|(target, _)| target);
        let rewritten = domain_target
            .is_some()
            .then(|| self.rewrite_records(message, domain, policy, lan))
            .transpose()?
            .unwrap_or(false);
        let filtered = (vpn_enabled || domain_target == Some(RouteTarget::Block))
            && (retain_ipv4_records(message.answers_mut())
                | retain_ipv4_records(message.additionals_mut()));
        if rewritten || filtered {
            message
                .answers_mut()
                .retain(|record| !record.record_type().is_dnssec());
            message
                .additionals_mut()
                .retain(|record| !record.record_type().is_dnssec());
            message
                .name_servers_mut()
                .retain(|record| !record.record_type().is_dnssec());
            message.set_authentic_data(false);
        }
        Ok(())
    }

    fn rewrite_records(
        &self,
        message: &mut Message,
        domain: &str,
        policy: &RoutingPolicy,
        lan: &LanContext,
    ) -> Result<bool> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| anyhow!("FakeDNS store lock poisoned"))?;
        let mut added = Vec::new();
        let names = relevant_names(message.answers(), domain);
        let result = (|| {
            let mut rewritten = rewrite_records(
                message.answers_mut(),
                domain,
                policy,
                &names,
                &mut store,
                &mut added,
                lan,
            )?;
            rewritten |= rewrite_records(
                message.additionals_mut(),
                domain,
                policy,
                &names,
                &mut store,
                &mut added,
                lan,
            )?;
            dataplane::install_mappings(&added)?;
            Ok(rewritten)
        })();
        match result {
            Ok(rewritten) => Ok(rewritten),
            Err(error) if added.is_empty() => Err(error),
            Err(error) => match store.remove_mappings(&added) {
                Ok(()) => Err(error),
                Err(rollback) => Err(anyhow!(
                    "DNS mapping failed: {error:#}; store rollback failed: {rollback:#}"
                )),
            },
        }
    }
}

fn rewrite_records(
    records: &mut [Record],
    domain: &str,
    policy: &RoutingPolicy,
    names: &HashSet<String>,
    store: &mut Store,
    added: &mut Vec<FakeMapping>,
    lan: &LanContext,
) -> Result<bool> {
    let mut rewritten = false;
    for record in records {
        if names.contains(&query_domain(&record.name().to_utf8()))
            && let RData::A(address) = record.data()
        {
            let real = address.0;
            // Fake DNAT to an on-link host gives replies an asymmetric return path.
            if is_lan_destination(real) || real == lan.address || lan.subnet.contains(&real) {
                continue;
            }
            let ttl = record.ttl().clamp(30, 3600);
            let (mapping, new) = store.allocate(domain, real, policy.target(domain, real), ttl)?;
            if new {
                added.push(mapping);
            }
            record.set_ttl(ttl);
            record.set_data(RData::A(A(mapping.fake)));
            rewritten = true;
        }
    }
    Ok(rewritten)
}

fn relevant_names(records: &[Record], domain: &str) -> HashSet<String> {
    let mut names = HashSet::from([domain.to_owned()]);
    let mut changed = true;
    while changed {
        changed = false;
        for record in records {
            if names.contains(&query_domain(&record.name().to_utf8()))
                && let RData::CNAME(name) = record.data()
            {
                changed |= names.insert(query_domain(&name.0.to_utf8()));
            }
        }
    }
    names
}

fn query_upstream(packet: &[u8], upstream: SocketAddr) -> Result<Vec<u8>> {
    let mark = dataplane::target_mark(RouteTarget::Direct);
    let socket = marked_socket(Type::DGRAM, Protocol::UDP, mark)?;
    socket.set_read_timeout(Some(DNS_TIMEOUT))?;
    socket.set_write_timeout(Some(DNS_TIMEOUT))?;
    let socket: UdpSocket = socket.into();
    socket.connect(upstream)?;
    socket.send(packet)?;
    let mut response = vec![0; MAX_DNS_PACKET];
    let length = socket.recv(&mut response)?;
    response.truncate(length);
    if Message::from_vec(&response).is_ok_and(|message| message.truncated()) {
        return query_upstream_tcp(packet, mark, upstream);
    }
    Ok(response)
}

fn retain_ipv4_records(records: &mut Vec<Record>) -> bool {
    let before = records.len();
    records.retain(|record| match record.data() {
        RData::AAAA(address) => {
            let ip = address.0;
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
        }
        RData::HTTPS(_) | RData::SVCB(_) => false,
        _ => true,
    });
    records.len() != before
}

fn panel_response(request: &Message) -> Result<Vec<u8>> {
    let mut response = Message::new();
    response
        .set_id(request.id())
        .set_message_type(MessageType::Response)
        .set_op_code(request.op_code())
        .set_recursion_desired(request.recursion_desired())
        .set_recursion_available(true)
        .set_response_code(ResponseCode::NoError)
        .add_queries(request.queries().iter().cloned());
    if let Some(query) = request
        .query()
        .filter(|query| query.query_type() == RecordType::A)
    {
        response.add_answer(Record::from_rdata(
            query.name().clone(),
            30,
            RData::A(A(PANEL_VIRTUAL_IP)),
        ));
    }
    response
        .to_vec()
        .context("failed to encode panel DNS response")
}

fn query_upstream_tcp(packet: &[u8], mark: u32, upstream: SocketAddr) -> Result<Vec<u8>> {
    let socket = marked_socket(Type::STREAM, Protocol::TCP, mark)?;
    socket.set_read_timeout(Some(DNS_TIMEOUT))?;
    socket.set_write_timeout(Some(DNS_TIMEOUT))?;
    socket.connect_timeout(&upstream.into(), DNS_TIMEOUT)?;
    let mut stream: TcpStream = socket.into();
    stream.write_all(&u16::try_from(packet.len())?.to_be_bytes())?;
    stream.write_all(packet)?;
    let mut length = [0; 2];
    stream.read_exact(&mut length)?;
    let mut response = vec![0; usize::from(u16::from_be_bytes(length))];
    stream.read_exact(&mut response)?;
    Ok(response)
}

fn marked_socket(kind: Type, protocol: Protocol, mark: u32) -> Result<Socket> {
    let socket = Socket::new(Domain::IPV4, kind, Some(protocol))?;
    #[cfg(target_os = "linux")]
    socket
        .set_mark(mark)
        .context("failed to set DNS egress mark")?;
    #[cfg(not(target_os = "linux"))]
    let _ = mark;
    Ok(socket)
}

fn empty_response(request: &Message, code: ResponseCode) -> Result<Vec<u8>> {
    let mut response = Message::new();
    response
        .set_id(request.id())
        .set_message_type(MessageType::Response)
        .set_op_code(request.op_code())
        .set_recursion_desired(request.recursion_desired())
        .set_recursion_available(true)
        .set_response_code(code)
        .add_queries(request.queries().iter().cloned());
    response.to_vec().context("failed to encode DNS response")
}

pub(super) fn failure_response(packet: &[u8]) -> Vec<u8> {
    Message::from_vec(packet)
        .and_then(|request| {
            empty_response(&request, ResponseCode::ServFail)
                .map_err(|error| hickory_proto::ProtoError::from(error.to_string()))
        })
        .unwrap_or_default()
}

fn query_domain(value: &str) -> String {
    normalize_domain(value)
        .unwrap_or_else(|_| value.trim().trim_end_matches('.').to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
