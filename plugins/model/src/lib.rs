use artist_plugin_sdk::artist::plugin::types::ModelConfig;
use artist_plugin_sdk::serde_json::Value;

struct Model;

fn configure(mut config: ModelConfig) -> Result<ModelConfig, String> {
    let mut parameters: Value = artist_plugin_sdk::serde_json::from_str(&config.parameters)
        .map_err(|error| format!("invalid model parameters: {error}"))?;
    let object = parameters
        .as_object_mut()
        .ok_or_else(|| "model parameters must be an object".to_owned())?;
    object.insert("artist_model_plugin".into(), Value::Bool(true));
    config.parameters = parameters.to_string();
    Ok(config)
}

artist_plugin_sdk::model_component!(Model, "artist.model", 0, configure);
