use anyhow::Result;

use super::{Change, update_config};
use crate::{AppState, config::normalize_routing, model::RoutingConfig};

pub(crate) fn update_routing(state: &AppState, mut routing: RoutingConfig) -> Result<()> {
    normalize_routing(&mut routing)?;
    update_config(state, |config| {
        config.routing = routing;
        Ok(Change::Policy)
    })
}
