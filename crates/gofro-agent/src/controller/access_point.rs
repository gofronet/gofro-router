use anyhow::{Result, anyhow, bail};

use crate::{
    AppState,
    config::validate_ssid,
    model::{ApInput, ApNetwork, WifiBand},
    network::update_ap,
};

pub(crate) fn update_access_point(state: &AppState, input: ApInput) -> Result<()> {
    validate_ssid(&input.ssid)?;
    let password = input
        .password
        .as_deref()
        .filter(|password| !password.is_empty());
    if password.is_some_and(|password| !(8..=63).contains(&password.len())) {
        bail!("пароль Wi-Fi должен содержать от 8 до 63 символов");
    }
    let _guard = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    update_ap(input.band, &input.ssid, password)?;
    update_cached_access_points(
        &mut state
            .access_points
            .lock()
            .map_err(|_| anyhow!("access point lock poisoned"))?,
        input.band,
        &input.ssid,
    );
    Ok(())
}

fn update_cached_access_points(networks: &mut [ApNetwork], band: Option<WifiBand>, ssid: &str) {
    for network in networks {
        if band.is_none_or(|band| network.band == band) {
            network.ssid = ssid.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_cached_access_point_names() {
        let mut networks = [
            ApNetwork {
                band: WifiBand::TwoGhz,
                ssid: "Two".into(),
            },
            ApNetwork {
                band: WifiBand::FiveGhz,
                ssid: "Five".into(),
            },
        ];
        update_cached_access_points(&mut networks, Some(WifiBand::FiveGhz), "New");
        assert_eq!(networks[0].ssid, "Two");
        assert_eq!(networks[1].ssid, "New");
        update_cached_access_points(&mut networks, None, "All");
        assert!(networks.iter().all(|network| network.ssid == "All"));
    }
}
