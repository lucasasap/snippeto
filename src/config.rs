use crate::snippet::{Snippet, Variable};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug)]
pub struct Config {
    pub snippets: Vec<Snippet>,
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let path = config_path();
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_config(&contents)
}

fn parse_config(contents: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let raw: RawConfig = serde_yaml_ng::from_str(contents)?;
    let mut snippets = Vec::with_capacity(raw.snippets.len() + raw.matches.len());
    let mut seen = HashSet::new();

    for (trigger, replacement) in raw.snippets {
        seen.insert(trigger.clone());
        snippets.push(Snippet::literal(trigger, replacement));
    }

    for item in raw.matches {
        if !seen.insert(item.trigger.clone()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("duplicate trigger `{}`", item.trigger),
            )
            .into());
        }

        let vars = item
            .vars
            .into_iter()
            .map(|var| match var {
                RawVar::Date { name, params } => Variable::Date {
                    name,
                    format: params.format,
                },
                RawVar::Shell { name, params } => Variable::Shell {
                    name,
                    cmd: params.cmd,
                },
            })
            .collect();

        snippets.push(Snippet::template(item.trigger, item.replace, vars));
    }

    Ok(Config { snippets })
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join(".config")
        .join("snippeto")
        .join("snippets.yml")
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    snippets: HashMap<String, String>,
    #[serde(default)]
    matches: Vec<RawMatch>,
}

#[derive(Debug, Deserialize)]
struct RawMatch {
    trigger: String,
    replace: String,
    #[serde(default)]
    vars: Vec<RawVar>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawVar {
    Date { name: String, params: DateParams },
    Shell { name: String, params: ShellParams },
}

#[derive(Debug, Deserialize)]
struct DateParams {
    format: String,
}

#[derive(Debug, Deserialize)]
struct ShellParams {
    cmd: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet_by_trigger<'a>(config: &'a Config, trigger: &str) -> &'a Snippet {
        config
            .snippets
            .iter()
            .find(|snippet| snippet.trigger() == trigger)
            .unwrap()
    }

    #[test]
    fn parses_legacy_snippets() {
        let config = parse_config(
            r#"
snippets:
  ";;email": "core.team@ymail.com"
"#,
        )
        .unwrap();

        assert_eq!(config.snippets.len(), 1);
        assert_eq!(
            snippet_by_trigger(&config, ";;email").render().unwrap(),
            "core.team@ymail.com"
        );
    }

    #[test]
    fn parses_espanso_style_matches() {
        let config = parse_config(
            r#"
matches:
  - trigger: ";;date"
    replace: "{{mydate}}"
    vars:
      - name: mydate
        type: date
        params:
          format: "%d/%m/%Y"
  - trigger: ";;seven"
    replace: "{{output}}"
    vars:
      - name: output
        type: shell
        params:
          cmd: "printf '01/01/2024 a 07/01/2024\n'"
"#,
        )
        .unwrap();

        let date = snippet_by_trigger(&config, ";;date").render().unwrap();
        assert_eq!(date.len(), 10);
        assert!(date.chars().all(|ch| ch.is_ascii_digit() || ch == '/'));

        assert_eq!(
            snippet_by_trigger(&config, ";;seven").render().unwrap(),
            "01/01/2024 a 07/01/2024"
        );
    }

    #[test]
    fn rejects_duplicate_triggers() {
        let error = parse_config(
            r#"
snippets:
  ";;date": "static"
matches:
  - trigger: ";;date"
    replace: "{{value}}"
    vars:
      - name: value
        type: shell
        params:
          cmd: "printf 'dynamic'"
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("duplicate trigger `;;date`"));
    }
}
