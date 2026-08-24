use artist_plugin_sdk::artist::plugin::types::{HookDecision, HookEvent};

struct Hooks;

fn observe(_: HookEvent) -> Result<HookDecision, String> {
    Ok(HookDecision::Proceed)
}

artist_plugin_sdk::hooks_component!(Hooks, "artist.hooks", observe);
