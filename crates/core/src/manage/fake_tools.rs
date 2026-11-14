//! What the real `claude` and `npx skills` leave on disk, so a test can plan again after an apply and see them as done.
//! They work in the folder the command's HOME names, as the real tools do.

use crate::harness::ExternalCommand;
use crate::testutil::link;
use serde_json::{json, Map, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn act_like_the_real_tool(c: &ExternalCommand) -> io::Result<()> {
    let Some((_, home)) = c.env.iter().find(|(key, _)| key == "HOME") else { return Ok(()) };
    let home = PathBuf::from(home);
    let args: Vec<&str> = c.args.iter().map(String::as_str).collect();
    match (c.program.as_str(), args.as_slice()) {
        ("claude", ["plugin", "marketplace", "add", source]) => add_marketplace(&home, source),
        ("claude", ["plugin", "install", plugin]) => install_plugin(&home, plugin),
        ("npx", ["--yes", "skills", "add", _, rest @ ..]) => {
            let from = rest.iter().position(|a| *a == "--skill").map_or(rest.len(), |at| at + 1);
            install_skills(&home, &rest[from..])
        }
        _ => Ok(()),
    }
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, value.to_string())
}

fn add_marketplace(home: &Path, source: &str) -> io::Result<()> {
    let name = source.rsplit(['/', '\\']).next().unwrap_or(source).trim_end_matches(".git");
    let origin = if source.starts_with("https://") {
        json!({"source": {"source": "git", "url": source}})
    } else {
        json!({"source": {"source": "github", "repo": source}})
    };
    let path = home.join(".claude").join("plugins").join("known_marketplaces.json");
    let mut known = read_object(&path);
    known.insert(name.to_owned(), origin);
    write_json(&path, &Value::Object(known))
}

fn install_plugin(home: &Path, plugin: &str) -> io::Result<()> {
    let path = home.join(".claude").join("plugins").join("installed_plugins.json");
    let mut installed = read_object(&path).remove("plugins").and_then(|p| p.as_object().cloned()).unwrap_or_default();
    installed.insert(plugin.to_owned(), json!([{"scope": "user"}]));
    write_json(&path, &json!({"version": 2, "plugins": installed}))
}

/// The skills CLI keeps the real folder under `.agents/skills` and links it from the Claude folder.
fn install_skills(home: &Path, names: &[&str]) -> io::Result<()> {
    for name in names {
        let real = home.join(".agents").join("skills").join(name);
        fs::create_dir_all(&real)?;
        fs::write(real.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\n"))?;
        link(&home.join(".claude").join("skills").join(name), &real);
    }
    Ok(())
}
