use anyhow::{Result, anyhow, bail};

use crate::{
    AppState,
    config::{save, validate_ssid},
    model::ApInput,
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
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let previous_ssid = std::mem::replace(&mut config.ap_ssid, input.ssid.clone());
    if let Err(error) = save(&state.config_path, &config) {
        config.ap_ssid = previous_ssid;
        return Err(error);
    }
    if let Err(error) = update_ap(&input.ssid, password) {
        config.ap_ssid = previous_ssid;
        return match save(&state.config_path, &config) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "access point update failed: {error:#}; configuration rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}
