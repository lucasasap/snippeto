use chrono::Local;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Snippet {
    trigger: String,
    expansion: Expansion,
}

#[derive(Debug, Clone)]
enum Expansion {
    Literal(String),
    Template(Template),
}

#[derive(Debug, Clone)]
struct Template {
    replace: String,
    vars: Vec<Variable>,
}

#[derive(Debug, Clone)]
pub enum Variable {
    Date { name: String, format: String },
    Shell { name: String, cmd: String },
}

impl Snippet {
    pub fn literal(trigger: String, replacement: String) -> Self {
        Self {
            trigger,
            expansion: Expansion::Literal(replacement),
        }
    }

    pub fn template(trigger: String, replace: String, vars: Vec<Variable>) -> Self {
        Self {
            trigger,
            expansion: Expansion::Template(Template { replace, vars }),
        }
    }

    pub fn trigger(&self) -> &str {
        &self.trigger
    }

    pub fn trigger_len(&self) -> usize {
        self.trigger.chars().count()
    }

    pub fn render(&self) -> Result<String, String> {
        self.expansion.render()
    }
}

impl Expansion {
    fn render(&self) -> Result<String, String> {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Template(template) => template.render(),
        }
    }
}

impl Template {
    fn render(&self) -> Result<String, String> {
        let mut values = HashMap::with_capacity(self.vars.len());

        for var in &self.vars {
            values.insert(var.name().to_owned(), var.evaluate()?);
        }

        Ok(render_template(&self.replace, &values))
    }
}

impl Variable {
    fn name(&self) -> &str {
        match self {
            Self::Date { name, .. } | Self::Shell { name, .. } => name,
        }
    }

    fn evaluate(&self) -> Result<String, String> {
        match self {
            Self::Date { format, .. } => Ok(Local::now().format(format).to_string()),
            Self::Shell { cmd, .. } => run_shell_command(cmd),
        }
    }
}

fn render_template(template: &str, values: &HashMap<String, String>) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];

        let Some(end) = after_start.find("}}") else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };

        let token = &after_start[..end];
        let key = token.trim();

        if let Some(value) = values.get(key) {
            rendered.push_str(value);
        } else {
            rendered.push_str("{{");
            rendered.push_str(token);
            rendered.push_str("}}");
        }

        rest = &after_start[end + 2..];
    }

    rendered.push_str(rest);
    rendered
}

fn run_shell_command(cmd: &str) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| format!("failed to run shell command `{cmd}`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(format!(
                "shell command `{cmd}` failed with {}",
                output.status
            ));
        }
        return Err(format!(
            "shell command `{cmd}` failed with {}: {stderr}",
            output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(strip_trailing_newline(stdout))
}

fn strip_trailing_newline(mut output: String) -> String {
    if output.ends_with('\n') {
        output.pop();
        if output.ends_with('\r') {
            output.pop();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_replaces_known_tokens_once() {
        let mut values = HashMap::new();
        values.insert("name".to_string(), "Alice".to_string());
        values.insert("nested".to_string(), "{{name}}".to_string());

        let rendered = render_template("Hello {{ name }} / {{nested}} / {{missing}}", &values);

        assert_eq!(rendered, "Hello Alice / {{name}} / {{missing}}");
    }

    #[test]
    fn shell_output_strips_one_trailing_newline() {
        let output = run_shell_command("printf 'hello\\n'").unwrap();
        assert_eq!(output, "hello");
    }
}
