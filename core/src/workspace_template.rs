//! Default sandbox template and bounded arguments for applying it inside a new sandbox.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Specification {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repos: Repositories,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Repositories {
    #[serde(default)]
    pub github: Vec<String>,
}

impl Specification {
    pub fn parse(source: &str) -> Result<Self, String> {
        let specification: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        if specification.name != "default" {
            return Err("The template name must be default.".into());
        }
        if specification.description.chars().count() > 160
            || specification.description.chars().any(char::is_control)
        {
            return Err("Use a single-line description of up to 160 characters.".into());
        }
        if specification.repos.github.len() > 10 {
            return Err("A template can contain up to 10 repositories.".into());
        }
        let mut destinations = HashSet::new();
        for repository in &specification.repos.github {
            let Some((owner, name)) = repository.split_once('/') else {
                return Err("Enter GitHub repositories as owner/repo.".into());
            };
            let valid_owner = !owner.is_empty()
                && owner.len() <= 39
                && owner
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !owner.starts_with('-')
                && !owner.ends_with('-');
            let valid_name = !name.is_empty()
                && name.len() <= 100
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                && name != "."
                && name != ".."
                && !name.to_ascii_lowercase().ends_with(".git");
            if !valid_owner || !valid_name {
                return Err(
                    "Enter GitHub repositories as owner/repo, without URLs or .git suffixes."
                        .into(),
                );
            }
            if !destinations.insert(name.to_ascii_lowercase()) {
                return Err(format!("Multiple repositories use the directory {name}."));
            }
        }
        Ok(specification)
    }

    pub fn source(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

pub const CLONE: &str = include_str!("../data/clone_template.py");
const LAUNCH: &str = "import base64,json,subprocess,sys\np=json.loads(base64.b64decode(''.join(sys.argv[1:])))\nsource=p.pop('source')\nsubprocess.Popen([sys.executable,'-c',source,base64.b64encode(json.dumps(p).encode()).decode()],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)\n";

pub fn arguments(config: &Value) -> anyhow::Result<Vec<String>> {
    let encoded = STANDARD.encode(serde_json::to_vec(config)?);
    let mut args = vec!["-c".into(), LAUNCH.into()];
    for chunk in encoded.as_bytes().chunks(1600) {
        args.push(String::from_utf8(chunk.to_vec())?);
    }
    anyhow::ensure!(
        args.len() <= 32,
        "template launch exceeds provider argument limit"
    );
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_round_trips_as_toml() {
        let input = "name = 'default'\ndescription = 'Backend'\n[repos]\ngithub = ['acme/api', 'acme/shared']\n";
        let template = Specification::parse(input).unwrap();
        let saved = Specification::parse(&template.source().unwrap()).unwrap();
        assert_eq!(saved.repos.github, ["acme/api", "acme/shared"]);
        assert_eq!(saved.description, "Backend");
        assert!(Specification::parse("name = 'default'")
            .unwrap()
            .repos
            .github
            .is_empty());
    }

    #[test]
    fn templates_allow_at_most_ten_repositories() {
        for count in [10, 11] {
            let source = Specification {
                name: "default".into(),
                description: String::new(),
                repos: Repositories {
                    github: (0..count).map(|n| format!("acme/repo{n}")).collect(),
                },
            }
            .source()
            .unwrap();
            assert_eq!(Specification::parse(&source).is_ok(), count == 10);
        }
    }

    #[test]
    fn invalid_templates_are_rejected() {
        for input in [
            "name = 'Default'",
            "name = 'backend'",
            "name = 'default'\nversion = 1",
            "name = 'default'\n[container]\nimage = 'alpine'",
            "name = 'default'\n[repos]\ngitlab = ['acme/api']",
        ] {
            assert!(Specification::parse(input).is_err(), "{input}");
        }
        for repo in [
            "https://github.com/acme/api",
            "acme/api.git",
            "acme/..",
            "acme/api/other",
            "-acme/api",
            "acme/$(command)",
        ] {
            let input = format!("name = 'default'\n[repos]\ngithub = ['{repo}']");
            assert!(Specification::parse(&input).is_err(), "{repo}");
        }
        assert!(Specification::parse(
            "name = 'default'\n[repos]\ngithub = ['acme/api', 'other/API']"
        )
        .is_err());
        for description in ["x".repeat(161), "line\nbreak".into()] {
            let source = Specification {
                name: "default".into(),
                description,
                repos: Repositories::default(),
            }
            .source()
            .unwrap();
            assert!(Specification::parse(&source).is_err());
        }
        let source = Specification {
            name: "default".into(),
            description: "é".repeat(160),
            repos: Repositories::default(),
        }
        .source()
        .unwrap();
        assert!(Specification::parse(&source).is_ok());
    }
}
