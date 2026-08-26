use anyhow::{Result, anyhow, bail};

use crate::{AppState, config::validate_ssid, model::ApInput, network::update_ap};

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
    update_ap(input.band, &input.ssid, password)
}
