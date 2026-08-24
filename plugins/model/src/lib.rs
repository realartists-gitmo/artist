use artist_plugin_sdk::artist::plugin::types::ModelConfig;

struct Model;

fn configure(config: ModelConfig) -> Result<ModelConfig, String> {
    Ok(config)
}

artist_plugin_sdk::model_component!(Model, "artist.model", configure);
