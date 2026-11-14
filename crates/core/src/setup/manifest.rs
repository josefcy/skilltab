//! `manifest.json`: parse any layout, render the exact bytes `backup.ps1` writes.

use crate::model::{Arg, FolderName, MarketplaceSource, SetupManifest};
use serde::Deserialize;

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    marketplaces: Vec<RawMarketplace>,
    #[serde(default, rename = "claudePlugins")]
    plugins: Vec<String>,
    #[serde(default, rename = "skillSources")]
    skill_sources: Vec<RawSource>,
    #[serde(default, rename = "ownSkills")]
    own_skills: Vec<String>,
}

#[derive(Deserialize)]
struct RawMarketplace {
    name: String,
    source: Option<RawOrigin>,
}

#[derive(Deserialize)]
struct RawOrigin {
    source: Option<String>,
    repo: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct RawSource {
    source: String,
    #[serde(default)]
    skills: Vec<String>,
}

impl SetupManifest {
    /// Accepts any JSON layout and key order, a leading BOM, and unknown keys. Every name that later
    /// becomes an argument or a path segment is validated. A marketplace source that is not github with a
    /// repo or git with a url is an error that names the marketplace. Restore reads a cloned manifest with it.
    pub(crate) fn parse(text: &str) -> Result<SetupManifest, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let raw: Raw = serde_json::from_str(text).map_err(|e| format!("The manifest is not valid: {e}."))?;
        let mut m = SetupManifest::default();

        for entry in raw.marketplaces {
            let name = &entry.name;
            let unsupported = || {
                format!("Marketplace \"{name}\" has a source skilltab cannot rebuild. Only github with a repo and git with a url are supported.")
            };
            let origin = entry.source.as_ref().ok_or_else(unsupported)?;
            let arg = |what: &str, v: &str| Arg::parse(v).map_err(|e| format!("Marketplace \"{name}\": {what} {e}."));
            let source = match (origin.source.as_deref(), &origin.repo, &origin.url) {
                (Some("github"), Some(repo), _) => MarketplaceSource::Github { repo: arg("repo", repo)? },
                (Some("git"), _, Some(url)) => MarketplaceSource::Git { url: arg("url", url)? },
                _ => return Err(unsupported()),
            };
            m.marketplaces.insert(Arg::parse(name).map_err(|e| format!("A marketplace name {e}."))?, source);
        }
        for plugin in raw.plugins {
            m.plugins.insert(Arg::parse(&plugin).map_err(|e| format!("A plugin {e}."))?);
        }
        for entry in raw.skill_sources {
            let source = Arg::parse(&entry.source).map_err(|e| format!("A skill source {e}."))?;
            let names = m.skill_sources.entry(source).or_default();
            for skill in entry.skills {
                names.insert(FolderName::parse(&skill).map_err(|e| format!("A skill name {e}."))?);
            }
        }
        for skill in raw.own_skills {
            m.own_skills.insert(FolderName::parse(&skill).map_err(|e| format!("An own skill name {e}."))?);
        }
        Ok(m)
    }

    /// The bytes `backup.ps1` writes: two-space layout, one entry per line, LF line ends, a final LF and no BOM.
    /// An empty list is `[]`, except claudePlugins and ownSkills, which hold a blank line.
    /// Strings escape only `\` and `"`, as its `Q` function does.
    pub(crate) fn render(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let marketplaces: Vec<String> = self
            .marketplaces
            .iter()
            .map(|(name, source)| {
                let origin = match source {
                    MarketplaceSource::Github { repo } => format!("\"source\": \"github\", \"repo\": {}", q(repo.as_str())),
                    MarketplaceSource::Git { url } => format!("\"source\": \"git\", \"url\": {}", q(url.as_str())),
                };
                format!("    {{ \"name\": {}, \"source\": {{ {origin} }} }}", q(name.as_str()))
            })
            .collect();
        let plugins: Vec<String> = self.plugins.iter().map(|p| format!("    {}", q(p.as_str()))).collect();
        let sources: Vec<String> = self
            .skill_sources
            .iter()
            .map(|(source, names)| {
                let names: Vec<String> = names.iter().map(|n| q(n.as_str())).collect();
                format!("    {{ \"source\": {}, \"skills\": [{}] }}", q(source.as_str()), names.join(", "))
            })
            .collect();
        let own: Vec<String> = self.own_skills.iter().map(|s| format!("    {}", q(s.as_str()))).collect();

        // The script feeds claudePlugins and ownSkills through `ForEach-Object`, so an empty list reaches its block
        // function as one null line and prints a blank line between the brackets. The other two print `[]`.
        let block = |key: &str, lines: &[String], quirk: bool, last: bool| {
            let comma = if last { "" } else { "," };
            match (lines.is_empty(), quirk) {
                (false, _) => format!("  \"{key}\": [
{}
  ]{comma}", lines.join(",
")),
                (true, true) => format!("  \"{key}\": [

  ]{comma}"),
                (true, false) => format!("  \"{key}\": []{comma}"),
            }
        };
        format!(
            "{{
{}
{}
{}
{}
}}
",
            block("marketplaces", &marketplaces, false, false),
            block("claudePlugins", &plugins, true, false),
            block("skillSources", &sources, false, false),
            block("ownSkills", &own, true, true),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../../tests/fixtures/setup/expected/manifest.json");
    const GOLDEN_EMPTY_LISTS: &str = include_str!("../../tests/fixtures/setup/empty-lists/expected/manifest.json");

    #[test]
    fn the_backup_ps1_output_parses_and_renders_back_to_the_same_bytes() {
        let m = SetupManifest::parse(GOLDEN).unwrap();
        assert_eq!(m.marketplaces.len(), 3);
        assert_eq!(m.render(), GOLDEN);

        let empty = SetupManifest::parse(GOLDEN_EMPTY_LISTS).unwrap();
        assert_eq!((empty.marketplaces.len(), empty.plugins.len(), empty.skill_sources.len(), empty.own_skills.len()), (1, 0, 0, 0));
        assert_eq!(empty.render(), GOLDEN_EMPTY_LISTS);
    }

    #[test]
    fn any_layout_order_bom_and_unknown_key_parses_to_the_same_manifest() {
        let loose = "\u{feff}{\"ownSkills\":[\"own-b\",\"Own-A\"],\"extra\":{\"x\":1},\"skillSources\":[{\"skills\":[\"wk\"],\"source\":\"https://example.test\"},\
            {\"source\":\"acme/skills\",\"skills\":[\"s-two\",\"Upper\",\"s-one\"],\"note\":1},{\"source\":\"https://example.test/g.git\",\"skills\":[\"gitty\"]}],\
            \"claudePlugins\":[\"plug-b@alpha-market\",\"plug-a@zeta-market\"],\"marketplaces\":[\
            {\"name\":\"zeta-market\",\"source\":{\"repo\":\"acme/zeta\",\"source\":\"github\"}},\
            {\"source\":{\"url\":\"https://example.test/acme/alpha.git\",\"source\":\"git\"},\"name\":\"alpha-market\"},\
            {\"name\":\"Beta-market\",\"source\":{\"source\":\"github\",\"repo\":\"acme/beta\"},\"ref\":\"main\"}]}";
        assert_eq!(SetupManifest::parse(loose).unwrap(), SetupManifest::parse(GOLDEN).unwrap());
    }

    #[test]
    fn an_empty_manifest_renders_the_empty_lists_as_backup_ps1_does_and_round_trips() {
        let empty = "{
  \"marketplaces\": [],
  \"claudePlugins\": [

  ],
  \"skillSources\": [],
  \"ownSkills\": [

  ]
}
";
        assert_eq!(SetupManifest::default().render(), empty);
        assert_eq!(SetupManifest::parse(empty).unwrap(), SetupManifest::default());
        let tidy = "{
  \"marketplaces\": [],
  \"claudePlugins\": [],
  \"skillSources\": [],
  \"ownSkills\": []
}
";
        assert_eq!(SetupManifest::parse(tidy).unwrap(), SetupManifest::default());
        assert_eq!(SetupManifest::parse("{}").unwrap(), SetupManifest::default());
    }

    #[test]
    fn each_list_is_empty_in_the_way_backup_ps1_writes_it_whatever_the_others_hold() {
        let all = SetupManifest::parse(GOLDEN).unwrap();
        let without = |plugins: bool, own: bool| {
            let mut m = all.clone();
            if !plugins {
                m.plugins.clear();
            }
            if !own {
                m.own_skills.clear();
            }
            m.render()
        };
        assert_eq!(without(true, true), GOLDEN);
        let no_plugins = without(false, true);
        assert!(no_plugins.contains("  \"claudePlugins\": [

  ],
  \"skillSources\": [
"), "{no_plugins}");
        assert!(no_plugins.contains("  \"ownSkills\": [
    \"Own-A\",
    \"own-b\"
  ]
}
"), "{no_plugins}");
        let no_own = without(true, false);
        assert!(no_own.contains("  \"claudePlugins\": [
    \"plug-a@zeta-market\",
"), "{no_own}");
        assert!(no_own.ends_with("  \"ownSkills\": [

  ]
}
"), "{no_own}");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped_and_nothing_else_is() {
        let m = SetupManifest::parse(r#"{"claudePlugins":["a\"b\\c@m\u00e9"]}"#).unwrap();
        assert!(m.render().contains("    \"a\\\"b\\\\c@m\u{e9}\"\n"), "{}", m.render());
        assert_eq!(SetupManifest::parse(&m.render()).unwrap(), m);
    }

    #[test]
    fn a_marketplace_the_format_cannot_rebuild_is_an_error_that_names_it() {
        for source in [r#"{"source":"directory","path":"C:\\x"}"#, r#"{"source":"github"}"#, r#"{"source":"git"}"#, "{}"] {
            let text = format!(r#"{{"marketplaces":[{{"name":"local-market","source":{source}}}]}}"#);
            let e = SetupManifest::parse(&text).unwrap_err();
            assert!(e.contains("local-market"), "{e}");
        }
        let e = SetupManifest::parse(r#"{"marketplaces":[{"name":"nosource"}]}"#).unwrap_err();
        assert!(e.contains("nosource"), "{e}");
    }

    #[test]
    fn a_hostile_manifest_is_refused_before_anything_can_use_it() {
        let hostile = [
            r#"{"claudePlugins":["--scope@x"]}"#,
            r#"{"skillSources":[{"source":"-y","skills":["a"]}]}"#,
            r#"{"skillSources":[{"source":"acme/skills","skills":["-y"]}]}"#,
            r#"{"skillSources":[{"source":"acme/skills","skills":[".."]}]}"#,
            r#"{"skillSources":[{"source":"acme/skills","skills":["a/b"]}]}"#,
            r#"{"ownSkills":["..\\up"]}"#,
            r#"{"ownSkills":["../up"]}"#,
            r#"{"ownSkills":["C:x"]}"#,
            r#"{"marketplaces":[{"name":"-m","source":{"source":"github","repo":"a/b"}}]}"#,
            r#"{"marketplaces":[{"name":"m","source":{"source":"git","url":"-oProxyCommand=x"}}]}"#,
            "not json",
        ];
        for text in hostile {
            assert!(SetupManifest::parse(text).is_err(), "{text}");
        }
    }
}
