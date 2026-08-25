use std::{
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
    model::RouteTarget,
    routing::RoutingPolicy,
};

use super::store::Store;

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
pub(super) const MAX_DNS_PACKET: usize = u16::MAX as usize;

pub(crate) struct FakeDns {
    store: Mutex<Store>,
    pub(super) updates: RwLock<()>,
    pub(super) active: AtomicBool,
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
    ) -> Result<Vec<u8>> {
        let request = Message::from_vec(packet).context("invalid DNS request")?;
        let query = request.query().context("DNS request has no question")?;
        let domain = query_domain(&query.name().to_utf8());
        let query_type = query.query_type();
        if matches!(
            query_type,
            RecordType::AAAA | RecordType::HTTPS | RecordType::SVCB
        ) {
            return empty_response(&request, ResponseCode::NoError);
        }

        let domain_target = policy.domain_target(&domain).map(|(target, _)| target);
        if domain_target == Some(RouteTarget::Block) {
            return empty_response(&request, ResponseCode::Refused);
        }
        let resolver_target = domain_target.unwrap_or_else(|| {
            if policy.config().default_target == RouteTarget::Block {
                RouteTarget::Vpn
            } else {
                policy.config().default_target
            }
        });
        let response = query_upstream(packet, resolver_target, upstream)?;
        let mut message = Message::from_vec(&response).context("invalid upstream DNS response")?;
        if message.message_type() != MessageType::Response
            || message.id() != request.id()
            || message.queries() != request.queries()
        {
            bail!("upstream DNS response does not match the request");
        }
        let rewritten = match domain_target {
            Some(target) => self.rewrite_records(&mut message, &domain, target)?,
            None => false,
        };
        message.answers_mut().retain(|record| {
            !matches!(
                record.record_type(),
                RecordType::AAAA | RecordType::HTTPS | RecordType::SVCB
            ) && (!rewritten || !record.record_type().is_dnssec())
        });
        message.additionals_mut().retain(|record| {
            !matches!(
                record.record_type(),
                RecordType::AAAA | RecordType::HTTPS | RecordType::SVCB
            ) && (!rewritten || !record.record_type().is_dnssec())
        });
        if rewritten {
            message
                .name_servers_mut()
                .retain(|record| !record.record_type().is_dnssec());
            message.set_authentic_data(false);
        }
        message.to_vec().context("failed to encode DNS response")
    }

    fn rewrite_records(
        &self,
        message: &mut Message,
        domain: &str,
        target: RouteTarget,
    ) -> Result<bool> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| anyhow!("FakeDNS store lock poisoned"))?;
        let mut added = Vec::new();
        let result = (|| {
            let mut rewritten = rewrite_records(
                message.answers_mut(),
                domain,
                target,
                &mut store,
                &mut added,
            )?;
            rewritten |= rewrite_records(
                message.additionals_mut(),
                domain,
                target,
                &mut store,
                &mut added,
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
    target: RouteTarget,
    store: &mut Store,
    added: &mut Vec<FakeMapping>,
) -> Result<bool> {
    let mut rewritten = false;
    for record in records {
        if let RData::A(address) = record.data() {
            let real = address.0;
            let ttl = record.ttl().clamp(30, 3600);
            let (mapping, new) = store.allocate(domain, real, target, ttl)?;
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

fn query_upstream(packet: &[u8], target: RouteTarget, upstream: SocketAddr) -> Result<Vec<u8>> {
    let mark = dataplane::target_mark(target);
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
mod tests {
    use super::*;

    #[test]
    fn overload_returns_servfail() {
        let mut request = Message::new();
        request.set_id(42).set_recursion_desired(true);

        let response = Message::from_vec(&failure_response(&request.to_vec().unwrap())).unwrap();

        assert_eq!(response.id(), 42);
        assert_eq!(response.response_code(), ResponseCode::ServFail);
    }

    #[test]
    fn accepts_dns_service_labels() {
        assert_eq!(
            query_domain("_Minecraft._TCP.Example.com."),
            "_minecraft._tcp.example.com"
        );
    }
}
