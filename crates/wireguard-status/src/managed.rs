use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const FRIEND_NAME_INPUT_LIMIT: usize = 1024;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FriendPeer {
    pub public_key: String,
    pub name: String,
    pub revoked: bool,
    pub can_share: bool,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManagedServerStatus {
    pub version: String,
    pub peers: Vec<FriendPeer>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FriendNameInput {
    pub name: String,
}

pub fn validate_name(input: &str) -> Result<String> {
    let name = input.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        bail!("friend name must be 1-60 non-control characters");
    }
    Ok(name.to_owned())
}

pub fn validate_peer_key(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 44
        || bytes[43] != b'='
        || !bytes[..43]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        bail!("invalid WireGuard public key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_and_wireguard_keys() {
        assert_eq!(validate_name("  Friend  ").unwrap(), "Friend");
        assert!(validate_name("\n").is_err());
        assert!(validate_name(&"a".repeat(61)).is_err());
        assert!(validate_peer_key("Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=").is_ok());
        assert!(validate_peer_key("not-a-key").is_err());
    }
}
