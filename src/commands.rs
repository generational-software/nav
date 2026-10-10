//! Commands taken from `Cargo.toml` and `package.json` in the open folder.

use std::fs;
use std::path::Path;

use serde_json::Value as Json;

/// A command named on a ring segment. `line` is the thing the name stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub line: String,
}

const MAX_COMMANDS: usize = 16;

/// Cargo commands first, then npm scripts. The same name is kept once, from Cargo.
pub fn read_dir(dir: &Path) -> Vec<Command> {
    let mut commands = Vec::new();
    if let Ok(text) = fs::read_to_string(dir.join("Cargo.toml")) {
        commands.extend(cargo_commands(&text));
    }
    if let Ok(text) = fs::read_to_string(dir.join("package.json")) {
        for command in package_commands(&text) {
            if commands.iter().any(|existing| existing.name == command.name) {
                continue;
            }
            commands.push(command);
        }
    }
    commands.truncate(MAX_COMMANDS);
    commands
}

fn cargo_commands(text: &str) -> Vec<Command> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(table) = value.as_table() else {
        return Vec::new();
    };
    let bins = target_names(table.get("bin"));
    let mut commands = Vec::new();
    if bins.is_empty() {
        if let Some(name) = package_bin(table) {
            commands.push(Command {
                name,
                line: "cargo run".to_string(),
            });
        }
    } else {
        for name in bins {
            commands.push(Command {
                name: name.clone(),
                line: format!("cargo run --bin {name}"),
            });
        }
    }
    for name in target_names(table.get("example")) {
        if commands.iter().any(|command| command.name == name) {
            continue;
        }
        commands.push(Command {
            name: name.clone(),
            line: format!("cargo run --example {name}"),
        });
    }
    commands
}

/// The package name is the implicit bin when Cargo would build one.
fn package_bin(table: &toml::map::Map<String, toml::Value>) -> Option<String> {
    let package = table.get("package")?.as_table()?;
    let autobins = package.get("autobins").and_then(|value| value.as_bool()).unwrap_or(true);
    if !autobins {
        return None;
    }
    let name = package.get("name").and_then(|value| value.as_str())?.trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

fn target_names(value: Option<&toml::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let items: Vec<&toml::Value> = if let Some(array) = value.as_array() {
        array.iter().collect()
    } else if value.is_table() {
        vec![value]
    } else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for item in items {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || names.iter().any(|existing| existing == name) {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

fn package_commands(text: &str) -> Vec<Command> {
    let Ok(Json::Object(root)) = serde_json::from_str::<Json>(text) else {
        return Vec::new();
    };
    let Some(Json::Object(scripts)) = root.get("scripts") else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for (name, body) in scripts {
        let Json::String(line) = body else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        commands.push(Command {
            name: name.to_string(),
            line: line.clone(),
        });
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(commands: &[Command]) -> Vec<&str> {
        commands.iter().map(|command| command.name.as_str()).collect()
    }

    #[test]
    fn cargo_lists_bins_then_examples_and_falls_back_to_the_package() {
        let listed = cargo_commands(
            r#"
            [package]
            name = "demo"
            [[bin]]
            name = "serve"
            [[example]]
            name = "demo"
            "#,
        );
        assert_eq!(names(&listed), ["serve", "demo"]);
        assert_eq!(listed[0].line, "cargo run --bin serve");
        assert_eq!(listed[1].line, "cargo run --example demo");

        let inline = cargo_commands(
            r#"
            bin = [{ name = "one" }, { name = "two" }]
            [package]
            name = "widget"
            "#,
        );
        assert_eq!(names(&inline), ["one", "two"]);

        let fallback = cargo_commands(
            r#"
            [package]
            name = "widget"
            [[example]]
            name = "sample"
            "#,
        );
        assert_eq!(names(&fallback), ["widget", "sample"]);
        assert_eq!(fallback[0].line, "cargo run");
        assert_eq!(fallback[1].line, "cargo run --example sample");

        assert!(cargo_commands(
            r#"
            [package]
            name = "widget"
            autobins = false
            "#
        )
        .is_empty());
        assert!(cargo_commands("this is not toml {").is_empty());
        assert!(cargo_commands("[workspace]\nmembers = [\"a\"]\n").is_empty());
    }

    #[test]
    fn package_scripts_keep_file_order_and_skip_a_bad_file() {
        let scripts = package_commands(
            r#"{"dependencies":{"left-pad":"1"},"scripts":{"dev":"vite","test":"vitest","broken":1}}"#,
        );
        assert_eq!(names(&scripts), ["dev", "test"]);
        assert_eq!(scripts[0].line, "vite");
        assert!(package_commands("not json").is_empty());
        assert!(package_commands(r#"{"name":"x"}"#).is_empty());
    }

    #[test]
    fn a_folder_merges_cargo_before_npm_and_stops_at_the_cap() {
        let dir = std::env::temp_dir().join(format!(
            "nav-commands-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"plaza\"\n\n[[bin]]\nname = \"serve\"\n\n[[example]]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"dev":"vite","serve":"node server.js","test":"vitest"}}"#,
        )
        .unwrap();
        let commands = read_dir(&dir);
        assert_eq!(names(&commands), ["serve", "demo", "dev", "test"]);
        assert_eq!(commands[0].line, "cargo run --bin serve");
        assert!(read_dir(Path::new("/tmp/nav-commands-missing-folder")).is_empty());

        let mut bins = String::from("[package]\nname = \"plaza\"\n");
        for index in 0..20 {
            bins.push_str(&format!("\n[[bin]]\nname = \"bin{index}\"\n"));
        }
        fs::write(dir.join("Cargo.toml"), bins).unwrap();
        fs::write(dir.join("package.json"), r#"{"scripts":{"late":"echo"}}"#).unwrap();
        let capped = read_dir(&dir);
        assert_eq!(capped.len(), MAX_COMMANDS);
        assert_eq!(capped[0].name, "bin0");
        assert!(capped.iter().all(|command| command.name != "late"));
        let _ = fs::remove_dir_all(&dir);
    }
}
