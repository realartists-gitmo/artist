struct Events;

fn observe(_: String) -> Result<(), String> {
    Ok(())
}

artist_plugin_sdk::events_component!(Events, "artist.events", 0, observe);
