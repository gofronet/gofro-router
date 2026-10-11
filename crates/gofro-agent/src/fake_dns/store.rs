use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

use crate::{
    dataplane::FakeMapping, model::RouteTarget, resources::ResourceLimits, routing::RoutingPolicy,
};

const FIRST_FAKE: u32 = u32::from_be_bytes([198, 18, 0, 1]);
const LAST_FAKE: u32 = u32::from_be_bytes([198, 19, 255, 254]);
// DNS TTLs are shorter than long-lived browser QUIC and TCP connections.
const CONNECTION_GRACE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
struct Lease {
    mapping: FakeMapping,
    expires: i64,
}

pub(super) struct Store {
    connection: Connection,
    leases: HashMap<(String, Ipv4Addr), Lease>,
    used: HashSet<Ipv4Addr>,
    next: u32,
    max_leases: usize,
}

impl Store {
    #[cfg(test)]
    pub(super) fn open(path: &Path) -> Result<Self> {
        Self::open_with_limits(path, ResourceLimits::STANDARD)
    }

    pub(super) fn open_with_limits(path: &Path, limits: ResourceLimits) -> Result<Self> {
        let connection =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        Self::from_connection_with_limits(connection, limits)
    }

    #[cfg(test)]
    pub(super) fn from_connection(connection: Connection) -> Result<Self> {
        Self::from_connection_with_limits(connection, ResourceLimits::STANDARD)
    }

    fn from_connection_with_limits(connection: Connection, limits: ResourceLimits) -> Result<Self> {
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
        connection.pragma_update(None, "cache_size", -(limits.sqlite_cache_kib as i32))?;
        connection.pragma_update(None, "wal_autocheckpoint", limits.wal_pages as u32)?;
        connection.pragma_update(None, "journal_size_limit", (limits.wal_pages * 4096) as u32)?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        connection.set_prepared_statement_cache_capacity(4);
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
                    let domain: String = row.get(1)?;
                    Ok((
                        (domain, real),
                        Lease {
                            mapping: FakeMapping { fake, real, target },
                            expires: row.get(4)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?
        };
        for lease in leases.values() {
            if !(FIRST_FAKE..=LAST_FAKE).contains(&u32::from(lease.mapping.fake)) {
                bail!(
                    "persisted fake IP {} is outside the allocation pool",
                    lease.mapping.fake
                );
            }
        }
        let used = leases.values().map(|lease| lease.mapping.fake).collect();
        Ok(Self {
            connection,
            leases,
            used,
            next: FIRST_FAKE,
            max_leases: limits.fake_leases,
        })
    }

    pub(super) fn reclassified(&self, policy: &RoutingPolicy) -> Vec<FakeMapping> {
        self.leases
            .iter()
            .map(|((domain, _), lease)| FakeMapping {
                target: policy.target(domain, lease.mapping.real),
                ..lease.mapping
            })
            .collect()
    }

    pub(super) fn commit_targets(&mut self, policy: &RoutingPolicy) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut update =
                transaction.prepare_cached("UPDATE fake_dns SET target = ?1 WHERE fake = ?2")?;
            for ((domain, _), lease) in &self.leases {
                let target = policy.target(domain, lease.mapping.real);
                update.execute(params![
                    target_code(target),
                    i64::from(u32::from(lease.mapping.fake))
                ])?;
            }
        }
        transaction.commit()?;
        for ((domain, _), lease) in &mut self.leases {
            lease.mapping.target = policy.target(domain, lease.mapping.real);
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
            if expires > lease.expires {
                self.connection
                    .prepare_cached("UPDATE fake_dns SET expires = ?1 WHERE fake = ?2")?
                    .execute(params![expires, i64::from(u32::from(lease.mapping.fake))])?;
                lease.expires = expires;
            }
            return Ok((lease.mapping, false));
        }

        // Never recycle a live fake IP or bypass policy to admit another lease.
        // Existing larger stores remain readable during upgrades and drain normally.
        anyhow::ensure!(
            self.leases.len() < self.max_leases,
            "лимит FakeDNS {} записей достигнут",
            self.max_leases
        );
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
        self.connection.prepare_cached(
            "INSERT INTO fake_dns (fake, domain, real, target, expires) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?.execute(params![
                i64::from(u32::from(fake)),
                domain,
                i64::from(u32::from(real)),
                target_code(target),
                expires
            ])?;
        self.used.insert(fake);
        self.leases.insert(key, Lease { mapping, expires });
        Ok((mapping, true))
    }

    pub(super) fn remove_mappings(&mut self, mappings: &[FakeMapping]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut delete = transaction.prepare_cached("DELETE FROM fake_dns WHERE fake = ?1")?;
            for mapping in mappings {
                delete.execute(params![i64::from(u32::from(mapping.fake))])?;
            }
        }
        transaction.commit()?;
        let removed = mappings
            .iter()
            .map(|mapping| mapping.fake)
            .collect::<HashSet<_>>();
        self.used.retain(|fake| !removed.contains(fake));
        self.leases
            .retain(|_, lease| !removed.contains(&lease.mapping.fake));
        if self.leases.capacity() > self.leases.len().saturating_mul(4).max(1024) {
            self.leases.shrink_to_fit();
            self.used.shrink_to_fit();
        }
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
    fn pool_wrap_and_reopen_never_allocate_panel_or_broadcast() {
        let mut store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        store.next = LAST_FAKE;
        let real = Ipv4Addr::new(1, 1, 1, 1);
        let (last, _) = store
            .allocate("last.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        let (first, _) = store
            .allocate("first.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        assert_eq!(last.fake, Ipv4Addr::from(LAST_FAKE));
        assert_eq!(first.fake, Ipv4Addr::from(FIRST_FAKE));
        let mut reopened = Store::from_connection(store.connection).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened
                .allocate("last.example", real, RouteTarget::Vpn, 60)
                .unwrap(),
            (last, false)
        );
        let (next, _) = reopened
            .allocate("next.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        assert_eq!(next.fake, Ipv4Addr::from(FIRST_FAKE + 1));
        assert!(!reopened.used.contains(&crate::model::PANEL_VIRTUAL_IP));
    }

    #[test]
    fn rejects_persisted_reserved_and_out_of_pool_rows() {
        for fake in [0, FIRST_FAKE - 1, LAST_FAKE + 1, u32::MAX] {
            let store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
            store.connection.execute(
                "INSERT INTO fake_dns (fake, domain, real, target, expires) VALUES (?1, 'bad.example', 1, 2, ?2)",
                params![i64::from(fake), i64::MAX],
            ).unwrap();
            let error = Store::from_connection(store.connection)
                .err()
                .expect("invalid pool row accepted");
            assert!(error.to_string().contains("outside the allocation pool"));
        }
    }

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

    #[test]
    fn failed_expiry_delete_preserves_rollback_mapping_and_lease() {
        let mut store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        let (mapping, _) = store
            .allocate(
                "vpn.example",
                "8.8.8.8".parse().unwrap(),
                RouteTarget::Vpn,
                60,
            )
            .unwrap();
        store.connection.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON fake_dns BEGIN SELECT RAISE(FAIL, 'read only'); END;").unwrap();
        let expired = store.expired_at(i64::MAX);
        assert!(store.remove_mappings(&expired).is_err());
        assert_eq!(store.expired_at(i64::MAX), vec![mapping]);
        assert_eq!(
            crate::dataplane::effective_target(expired[0].target, false),
            RouteTarget::Direct
        );
    }

    #[test]
    fn full_store_renews_existing_leases_and_never_recycles_live_addresses() {
        let mut store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        store.max_leases = 2;
        let real = Ipv4Addr::new(8, 8, 8, 8);
        let (first, _) = store
            .allocate("first.example", real, RouteTarget::Vpn, 3600)
            .unwrap();
        let (second, _) = store
            .allocate("second.example", real, RouteTarget::Direct, 60)
            .unwrap();
        assert!(
            store
                .allocate("third.example", real, RouteTarget::Vpn, 60)
                .is_err()
        );
        assert_eq!(store.len(), 2);
        let expires = store.leases[&("first.example".into(), real)].expires;
        assert_eq!(
            store
                .allocate("first.example", real, RouteTarget::Vpn, 30)
                .unwrap(),
            (first, false)
        );
        assert_eq!(
            store.leases[&("first.example".into(), real)].expires,
            expires
        );
        store.remove_mappings(&[second]).unwrap();
        let (third, _) = store
            .allocate("third.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        assert_ne!(third.fake, first.fake);
    }

    #[test]
    fn sustained_writes_keep_wal_small_and_reopen_preserves_mappings() {
        let root = std::env::temp_dir().join(format!(
            "gofro-wal-{}-{}",
            std::process::id(),
            unix_time().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("routing.sqlite");
        let mut store = Store::open(&path).unwrap();
        for index in 0..2000 {
            store
                .allocate(
                    &format!("domain-{index}.example"),
                    Ipv4Addr::new(1, 1, 1, 1),
                    RouteTarget::Vpn,
                    60,
                )
                .unwrap();
        }
        assert!(
            std::fs::metadata(root.join("routing.sqlite-wal"))
                .unwrap()
                .len()
                < 512 * 1024
        );
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            -256
        );
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.len(), 2000);
        assert!(
            !reopened
                .allocate(
                    "domain-999.example",
                    Ipv4Addr::new(1, 1, 1, 1),
                    RouteTarget::Vpn,
                    60
                )
                .unwrap()
                .1
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compact_store_uses_small_targets_and_preserves_larger_upgrade_state() {
        let connection = Connection::open_in_memory().unwrap();
        let mut store =
            Store::from_connection_with_limits(connection, ResourceLimits::COMPACT).unwrap();
        assert_eq!(store.max_leases, 8192);
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            -128
        );
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            32
        );
        let real = Ipv4Addr::new(1, 1, 1, 1);
        let (first, _) = store
            .allocate("first.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        store
            .allocate("second.example", real, RouteTarget::Vpn, 60)
            .unwrap();
        let mut limits = ResourceLimits::COMPACT;
        limits.fake_leases = 1;
        let mut reopened = Store::from_connection_with_limits(store.connection, limits).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened
                .allocate("first.example", real, RouteTarget::Vpn, 60)
                .unwrap(),
            (first, false)
        );
        assert!(
            reopened
                .allocate("third.example", real, RouteTarget::Vpn, 60)
                .is_err()
        );
    }
}
