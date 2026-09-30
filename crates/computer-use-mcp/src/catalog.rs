//! The built-in skills as MCP **prompts** and **resources**, so clients can
//! list and read the per-OS how-to playbooks without calling a tool.
//!
//! * `prompts/list`, `prompts/get` — one prompt per skill; getting it returns
//!   the playbook as a user message (clients surface these as slash commands
//!   or attachable context).
//! * `resources/list`, `resources/read` — `computer-use://skills/<name>`,
//!   Markdown.
//!
//! Both follow `skills = true|false` in the settings, like the `skill` tool.

use computer_use::skills::{Os, Skill, skills_for};
use serde_json::{Value, json};

use crate::jsonrpc::INVALID_PARAMS;

/// MCP: resource not found.
pub const RESOURCE_NOT_FOUND: i64 = -32002;

const URI_PREFIX: &str = "computer-use://skills/";
const MIME: &str = "text/markdown";

/// A JSON-RPC error: code and message.
pub type RpcError = (i64, String);

/// The `capabilities` an initialize reply advertises for `tools` plus (when
/// skills are on) prompts and resources.
pub fn capabilities(skills: bool, tools_list_changed: bool) -> Value {
    let mut c = json!({"tools": {"listChanged": tools_list_changed}});
    if skills {
        c["prompts"] = json!({"listChanged": false});
        c["resources"] = json!({"subscribe": false, "listChanged": false});
    }
    c
}

fn skills(enabled: bool) -> &'static [Skill] {
    if enabled {
        skills_for(Os::current())
    } else {
        &[]
    }
}

fn title(skill: &Skill) -> String {
    skill
        .body
        .lines()
        .next()
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| skill.name.to_string())
}

pub fn prompts_list(enabled: bool) -> Value {
    let prompts: Vec<Value> = skills(enabled)
        .iter()
        .map(|s| json!({"name": s.name, "title": title(s), "description": s.summary}))
        .collect();
    json!({"prompts": prompts})
}

pub fn prompts_get(enabled: bool, params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "prompts/get requires `name`".to_string()))?;
    let skill = skills(enabled)
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| (INVALID_PARAMS, format!("unknown prompt: {name}")))?;
    Ok(json!({
        "description": skill.summary,
        "messages": [{
            "role": "user",
            "content": {"type": "text", "text": skill.body},
        }],
    }))
}

pub fn resources_list(enabled: bool) -> Value {
    let resources: Vec<Value> = skills(enabled)
        .iter()
        .map(|s| {
            json!({
                "uri": format!("{URI_PREFIX}{}", s.name),
                "name": s.name,
                "title": title(s),
                "description": s.summary,
                "mimeType": MIME,
            })
        })
        .collect();
    json!({"resources": resources})
}

pub fn resources_read(enabled: bool, params: &Value) -> Result<Value, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "resources/read requires `uri`".to_string()))?;
    let skill = uri
        .strip_prefix(URI_PREFIX)
        .and_then(|name| skills(enabled).iter().find(|s| s.name == name))
        .ok_or_else(|| (RESOURCE_NOT_FOUND, format!("resource not found: {uri}")))?;
    Ok(json!({
        "contents": [{"uri": uri, "mimeType": MIME, "text": skill.body}],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_skills_of_this_os_as_prompts_and_resources() {
        let prompts = prompts_list(true);
        let names: Vec<&str> = prompts["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"files") && names.contains(&"vscode"),
            "{names:?}"
        );
        let resources = resources_list(true);
        assert_eq!(
            resources["resources"].as_array().unwrap().len(),
            names.len()
        );
        assert_eq!(resources["resources"][0]["mimeType"], "text/markdown");
    }

    #[test]
    fn get_and_read_return_the_playbook() {
        let got = prompts_get(true, &json!({"name": "files"})).unwrap();
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.starts_with("# "), "{text}");
        let read = resources_read(true, &json!({"uri": "computer-use://skills/files"})).unwrap();
        assert_eq!(read["contents"][0]["text"].as_str().unwrap(), text);
        assert_eq!(read["contents"][0]["uri"], "computer-use://skills/files");
    }

    #[test]
    fn errors_use_the_spec_codes() {
        assert_eq!(prompts_get(true, &json!({})).unwrap_err().0, INVALID_PARAMS);
        assert_eq!(
            prompts_get(true, &json!({"name": "nope"})).unwrap_err().0,
            INVALID_PARAMS
        );
        assert_eq!(
            resources_read(true, &json!({"uri": "computer-use://skills/nope"}))
                .unwrap_err()
                .0,
            RESOURCE_NOT_FOUND
        );
        assert_eq!(
            resources_read(true, &json!({"uri": "file:///etc/passwd"}))
                .unwrap_err()
                .0,
            RESOURCE_NOT_FOUND
        );
    }

    #[test]
    fn switching_skills_off_empties_everything() {
        assert!(
            prompts_list(false)["prompts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            resources_list(false)["resources"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(prompts_get(false, &json!({"name": "files"})).is_err());
        assert!(resources_read(false, &json!({"uri": "computer-use://skills/files"})).is_err());
        let off = capabilities(false, true);
        assert!(off.get("prompts").is_none() && off.get("resources").is_none());
        let on = capabilities(true, false);
        assert!(on.get("prompts").is_some() && on.get("resources").is_some());
    }
}
