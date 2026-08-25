use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

use crate::{dataplane::FakeMapping, model::RouteTarget, routing::RoutingPolicy};

const FIRST_FAKE: u32 = u32::from_be_bytes([198, 18, 0, 1]);
const LAST_FAKE: u32 = u32::from_be_bytes([198, 19, 255, 254]);
// DNS TTLs are shorter than long-lived browser QUIC and TCP connections.
const CONNECTION_GRACE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
struct Lease {
    domain: String,
    mapping: FakeMapping,
    expires: i64,
}

pub(super) struct Store {
    connection: Connection,
    leases: HashMap<(String, Ipv4Addr), Lease>,
    used: HashSet<Ipv4Addr>,
    next: u32,
}

impl Store {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS fake_dns (
               fake INTEGER PRIMARY KEY,
               domain TEXT NOT NULL,
               real INTEGER NOT NULL,
               target INTEGER NOT NULL,
               expires INTEGER NOT NULL,
               UNIQUE(domain, real)
             );",
        )?;
        let now = unix_time()?;
        connection.execute(
            "DELETE FROM fake_dns WHERE expires <= ?1",
            params![now.saturating_sub(CONNECTION_GRACE_SECONDS)],
        )?;
        let leases = {
            let mut statement = connection.prepare(
                "SELECT fake, domain, real, target, expires FROM fake_dns ORDER BY fake",
            )?;
            statement
                .query_map([], |row| {
                    let fake = ipv4_from_sql(row.get(0)?)?;
                    let real = ipv4_from_sql(row.get(2)?)?;
                    let target = target_from_code(row.get(3)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            error.into(),
                        )
                    })?;
                    Ok(Lease {
                        domain: row.get(1)?,
                        mapping: FakeMapping { fake, real, target },
                        expires: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let used = leases.iter().map(|lease| lease.mapping.fake).collect();
        let leases = leases
            .into_iter()
            .map(|lease| ((lease.domain.clone(), lease.mapping.real), lease))
            .collect();
        Ok(Self {
            connection,
            leases,
            used,
            next: FIRST_FAKE,
        })
    }

    pub(super) fn reclassified(&self, policy: &RoutingPolicy) -> Vec<FakeMapping> {
        self.leases
            .values()
            .map(|lease| FakeMapping {
                target: policy.target(&lease.domain, lease.mapping.real),
                ..lease.mapping
            })
            .collect()
    }

    pub(super) fn commit_targets(&mut self, policy: &RoutingPolicy) -> Result<()> {
        let updates = self
            .leases
            .iter()
            .map(|(key, lease)| {
                (
                    key.clone(),
                    lease.mapping.fake,
                    policy.target(&lease.domain, lease.mapping.real),
                )
            })
            .collect::<Vec<_>>();
        let transaction = self.connection.transaction()?;
        for (_, fake, target) in &updates {
            transaction.execute(
                "UPDATE fake_dns SET target = ?1 WHERE fake = ?2",
                params![target_code(*target), i64::from(u32::from(*fake))],
            )?;
        }
        transaction.commit()?;
        for (key, _, target) in updates {
            if let Some(lease) = self.leases.get_mut(&key) {
                lease.mapping.target = target;
            }
        }
        Ok(())
    }

    pub(super) fn len(&self) -> usize {
        self.leases.len()
    }

    pub(super) fn expired(&self) -> Result<Vec<FakeMapping>> {
        Ok(self.expired_at(unix_time()?))
    }

    fn expired_at(&self, now: i64) -> Vec<FakeMapping> {
        let cutoff = now.saturating_sub(CONNECTION_GRACE_SECONDS);
        self.leases
            .values()
            .filter(|lease| lease.expires <= cutoff)
            .map(|lease| lease.mapping)
            .collect()
    }

    pub(super) fn allocate(
        &mut self,
        domain: &str,
        real: Ipv4Addr,
        target: RouteTarget,
        ttl: u32,
    ) -> Result<(FakeMapping, bool)> {
        let key = (domain.to_owned(), real);
        let expires = unix_time()? + i64::from(ttl.clamp(30, 3600)) + 300;
        if let Some(lease) = self.leases.get_mut(&key) {
            lease.expires = expires;
            self.connection.execute(
                "UPDATE fake_dns SET expires = ?1 WHERE fake = ?2",
                params![expires, i64::from(u32::from(lease.mapping.fake))],
            )?;
            return Ok((lease.mapping, false));
        }

        // ponytail: the /15 holds 131k leases; rebuild-on-exhaustion if real traffic reaches it.
        let pool_size = LAST_FAKE - FIRST_FAKE + 1;
        let fake = (0..pool_size)
            .map(|offset| FIRST_FAKE + (self.next - FIRST_FAKE + offset) % pool_size)
            .map(Ipv4Addr::from)
            .find(|candidate| !self.used.contains(candidate))
            .context("пул FakeDNS 198.18.0.0/15 исчерпан")?;
        self.next = if u32::from(fake) == LAST_FAKE {
            FIRST_FAKE
        } else {
            u32::from(fake) + 1
        };
        let mapping = FakeMapping { fake, real, target };
        self.connection.execute(
            "INSERT INTO fake_dns (fake, domain, real, target, expires) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                i64::from(u32::from(fake)),
                domain,
                i64::from(u32::from(real)),
                target_code(target),
                expires
            ],
        )?;
        self.used.insert(fake);
        self.leases.insert(
            key,
            Lease {
                domain: domain.to_owned(),
                mapping,
                expires,
            },
        );
        Ok((mapping, true))
    }

    pub(super) fn remove_mappings(&mut self, mappings: &[FakeMapping]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for mapping in mappings {
            transaction.execute(
                "DELETE FROM fake_dns WHERE fake = ?1",
                params![i64::from(u32::from(mapping.fake))],
            )?;
        }
        transaction.commit()?;
        let removed = mappings
            .iter()
            .map(|mapping| mapping.fake)
            .collect::<HashSet<_>>();
        self.used.retain(|fake| !removed.contains(fake));
        self.leases
            .retain(|_, lease| !removed.contains(&lease.mapping.fake));
        Ok(())
    }
}

fn unix_time() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    i64::try_from(seconds).context("Unix time exceeds SQLite integer range")
}

fn target_code(target: RouteTarget) -> i64 {
    match target {
        RouteTarget::Direct => 1,
        RouteTarget::Vpn => 2,
        RouteTarget::Block => 3,
    }
}

fn target_from_code(code: i64) -> Result<RouteTarget> {
    match code {
        1 => Ok(RouteTarget::Direct),
        2 => Ok(RouteTarget::Vpn),
        3 => Ok(RouteTarget::Block),
        _ => bail!("invalid persisted route target {code}"),
    }
}

fn ipv4_from_sql(value: i64) -> rusqlite::Result<Ipv4Addr> {
    u32::try_from(value).map(Ipv4Addr::from).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, error.into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_fake_ip_leases() {
        let connection = Connection::open_in_memory().unwrap();
        let mut store = Store::from_connection(connection).unwrap();
        let (first, new) = store
            .allocate(
                "example.com",
                "1.1.1.1".parse().unwrap(),
                RouteTarget::Vpn,
                60,
            )
            .unwrap();
        let (again, new_again) = store
            .allocate(
                "example.com",
                "1.1.1.1".parse().unwrap(),
                RouteTarget::Vpn,
                60,
            )
            .unwrap();
        assert!(new);
        assert!(!new_again);
        assert_eq!(first, again);
        assert_eq!(first.fake, Ipv4Addr::new(198, 18, 0, 1));
    }

    #[test]
    fn expired_leases_return_addresses_to_pool() {
        let connection = Connection::open_in_memory().unwrap();
        let mut store = Store::from_connection(connection).unwrap();
        let (first, _) = store
            .allocate(
                "old.example",
                "1.1.1.1".parse().unwrap(),
                RouteTarget::Vpn,
                60,
            )
            .unwrap();

        let expired = store.expired_at(i64::MAX);
        store.remove_mappings(&expired).unwrap();
        store.next = FIRST_FAKE;
        let (replacement, _) = store
            .allocate(
                "new.example",
                "8.8.8.8".parse().unwrap(),
                RouteTarget::Direct,
                60,
            )
            .unwrap();

        assert_eq!(expired, vec![first]);
        assert_eq!(replacement.fake, first.fake);
    }

    #[test]
    fn keeps_mapping_for_long_lived_connections() {
        let connection = Connection::open_in_memory().unwrap();
        let mut store = Store::from_connection(connection).unwrap();
        let (mapping, _) = store
            .allocate(
                "video.example",
                "1.1.1.1".parse().unwrap(),
                RouteTarget::Vpn,
                60,
            )
            .unwrap();
        let expires = store.leases.values().next().unwrap().expires;

        assert!(
            store
                .expired_at(expires + CONNECTION_GRACE_SECONDS - 1)
                .is_empty()
        );
        assert_eq!(
            store.expired_at(expires + CONNECTION_GRACE_SECONDS),
            vec![mapping]
        );
    }
}
