use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub snippets: HashMap<String, String>,
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let path = config_path();
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        format!("cannot read {}: {e}", path.display())
    })?;
    let config: Config = serde_yaml_ng::from_str(&contents)?;
    Ok(config)
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join(".config")
        .join("snippeto")
        .join("snippets.yml")
}
