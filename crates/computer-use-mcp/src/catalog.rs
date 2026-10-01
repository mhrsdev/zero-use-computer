//! The skills (`skills/`) as MCP **prompts** and **resources**, for clients
//! that can't load skill files: Cursor, VS Code, Codex, a remote agent.
//!
//! * `prompts/list`, `prompts/get`: one prompt per skill (its `SKILL.md`
//!   without the front matter), sent as a user message. Clients show these
//!   as slash commands or attachable context.
//! * `resources/list`, `resources/read`: every file of every skill under
//!   `computer-use://skills/<skill>/<path>`, Markdown. A skill's links to its
//!   `reference/*.md` resolve against its own URI, so an agent reads a
//!   reference file only when it needs it.
//!
//! None of this changes the tool list, so prompt caching is unaffected.

use std::borrow::Cow;

use serde_json::{Value, json};

use crate::jsonrpc::INVALID_PARAMS;

/// MCP: resource not found.
pub const RESOURCE_NOT_FOUND: i64 = -32002;

const URI_PREFIX: &str = "computer-use://skills/";
const MIME: &str = "text/markdown";

/// A JSON-RPC error: code and message.
pub type RpcError = (i64, String);

/// One file of a skill, embedded at build time.
struct File {
    skill: &'static str,
    path: &'static str,
    /// As checked out: a Windows checkout may have CRLF line endings.
    raw: &'static str,
}

const FILES: &[File] = &[
    File {
        skill: "computer-use",
        path: "SKILL.md",
        raw: include_str!("../../../skills/computer-use/SKILL.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/screens.md",
        raw: include_str!("../../../skills/computer-use/reference/screens.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/tools.md",
        raw: include_str!("../../../skills/computer-use/reference/tools.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/special-content.md",
        raw: include_str!("../../../skills/computer-use/reference/special-content.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/drawing.md",
        raw: include_str!("../../../skills/computer-use/reference/drawing.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/apps/browsers.md",
        raw: include_str!("../../../skills/computer-use/reference/apps/browsers.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/apps/office.md",
        raw: include_str!("../../../skills/computer-use/reference/apps/office.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/apps/mail-and-chat.md",
        raw: include_str!("../../../skills/computer-use/reference/apps/mail-and-chat.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/apps/vscode.md",
        raw: include_str!("../../../skills/computer-use/reference/apps/vscode.md"),
    },
    File {
        skill: "computer-use",
        path: "reference/apps/image-editors.md",
        raw: include_str!("../../../skills/computer-use/reference/apps/image-editors.md"),
    },
    File {
        skill: "computer-use-security",
        path: "SKILL.md",
        raw: include_str!("../../../skills/computer-use-security/SKILL.md"),
    },
    File {
        skill: "computer-use-security",
        path: "reference/examples.md",
        raw: include_str!("../../../skills/computer-use-security/reference/examples.md"),
    },
];

impl File {
    fn uri(&self) -> String {
        format!("{URI_PREFIX}{}/{}", self.skill, self.path)
    }

    fn is_main(&self) -> bool {
        self.path == "SKILL.md"
    }

    /// The file with `\n` line endings, however it was checked out.
    fn text(&self) -> Cow<'static, str> {
        if self.raw.contains('\r') {
            Cow::Owned(self.raw.replace("\r\n", "\n"))
        } else {
            Cow::Borrowed(self.raw)
        }
    }

    /// The text after the YAML front matter.
    fn body(&self) -> String {
        front_matter(&self.text()).1.to_string()
    }

    /// The skill's `description`, or the file's first heading.
    fn description(&self) -> String {
        let text = self.text();
        let (meta, body) = front_matter(&text);
        description(meta).unwrap_or_else(|| title(body))
    }
}

/// Split `---\n<yaml>\n---\n<body>` into (yaml, body). A Windows checkout
/// may have CRLF line endings.
fn front_matter(text: &str) -> (&str, &str) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return ("", text);
    };
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..at], rest[at + line.len()..].trim_start());
        }
        at += line.len();
    }
    ("", text)
}

/// The `description:` of the front matter, folded (`>-`) or on one line.
fn description(meta: &str) -> Option<String> {
    let mut lines = meta.lines();
    let first = lines.find_map(|l| l.strip_prefix("description:"))?.trim();
    let text = if first.starts_with('>') || first.starts_with('|') {
        lines
            .take_while(|l| l.starts_with(' '))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        first.to_string()
    };
    Some(text).filter(|t| !t.is_empty())
}

fn title(body: &str) -> String {
    body.lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The `capabilities` an initialize reply advertises.
pub fn capabilities(tools_list_changed: bool) -> Value {
    json!({
        "tools": {"listChanged": tools_list_changed},
        "prompts": {"listChanged": false},
        "resources": {"subscribe": false, "listChanged": false},
    })
}

pub fn prompts_list() -> Value {
    let prompts: Vec<Value> = FILES
        .iter()
        .filter(|f| f.is_main())
        .map(|f| {
            json!({
                "name": f.skill,
                "title": title(&f.body()),
                "description": f.description(),
            })
        })
        .collect();
    json!({ "prompts": prompts })
}

pub fn prompts_get(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "prompts/get requires `name`".to_string()))?;
    let main = |skill: &str| FILES.iter().find(|f| f.is_main() && f.skill == skill);
    let file = main(name).ok_or_else(|| (INVALID_PARAMS, format!("unknown prompt: {name}")))?;
    // The guide never comes without the safety rules it relies on.
    let mut files = vec![file];
    if name == "computer-use" {
        files.extend(main("computer-use-security"));
    }
    let messages: Vec<Value> = files
        .iter()
        .map(|f| {
            let text = format!(
                "{}\n\nLinked files are MCP resources under {URI_PREFIX}{}/ (resources/read).",
                f.body().trim_end(),
                f.skill
            );
            json!({"role": "user", "content": {"type": "text", "text": text}})
        })
        .collect();
    Ok(json!({
        "description": file.description(),
        "messages": messages,
    }))
}

pub fn resources_list() -> Value {
    let resources: Vec<Value> = FILES
        .iter()
        .map(|f| {
            json!({
                "uri": f.uri(),
                "name": format!("{}/{}", f.skill, f.path),
                "title": title(&f.body()),
                "description": f.description(),
                "mimeType": MIME,
                "size": f.text().len(),
            })
        })
        .collect();
    json!({ "resources": resources })
}

pub fn resources_read(params: &Value) -> Result<Value, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "resources/read requires `uri`".to_string()))?;
    let file = FILES
        .iter()
        .find(|f| f.uri() == uri)
        .ok_or_else(|| (RESOURCE_NOT_FOUND, format!("resource not found: {uri}")))?;
    Ok(json!({
        "contents": [{"uri": uri, "mimeType": MIME, "text": file.text()}],
    }))
}

/// Answer a prompts/resources request, or `None` if `method` is another one.
pub fn handle(method: &str, params: &Value) -> Option<Result<Value, RpcError>> {
    Some(match method {
        "prompts/list" => Ok(prompts_list()),
        "prompts/get" => prompts_get(params),
        "resources/list" => Ok(resources_list()),
        "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
        "resources/read" => resources_read(params),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_is_a_prompt_and_every_file_a_resource() {
        let prompts = prompts_list();
        let names: Vec<&str> = prompts["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["computer-use", "computer-use-security"]);
        assert!(
            prompts["prompts"][1]["description"]
                .as_str()
                .unwrap()
                .starts_with("Safety rules for the computer-use tools")
        );
        let resources = resources_list();
        let list = resources["resources"].as_array().unwrap();
        assert_eq!(list.len(), FILES.len());
        assert!(list.iter().all(|r| r["mimeType"] == "text/markdown"));
        assert!(
            list.iter()
                .any(|r| r["uri"] == "computer-use://skills/computer-use/reference/tools.md")
        );
    }

    #[test]
    fn get_and_read_return_the_skill() {
        let got = prompts_get(&json!({"name": "computer-use-security"})).unwrap();
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.starts_with("# Computer use: security rules"), "{text}");
        assert!(!text.contains("name: computer-use-security"));
        assert!(text.ends_with("computer-use://skills/computer-use-security/ (resources/read)."));

        let uri = "computer-use://skills/computer-use/SKILL.md";
        let read = resources_read(&json!({ "uri": uri })).unwrap();
        assert_eq!(read["contents"][0]["uri"], uri);
        let md = read["contents"][0]["text"].as_str().unwrap();
        assert!(md.starts_with("---\nname: computer-use\n"));
        // Its relative links name resources that exist.
        for link in ["reference/screens.md", "reference/tools.md"] {
            assert!(md.contains(&format!("]({link})")), "{link}");
            let uri = format!("computer-use://skills/computer-use/{link}");
            assert!(resources_read(&json!({ "uri": uri })).is_ok(), "{uri}");
        }
    }

    #[test]
    fn every_skill_file_is_served() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        let mut on_disk = Vec::new();
        let mut dirs = vec![root.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|e| e == "md") {
                    let rel = p
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    on_disk.push(rel);
                }
            }
        }
        on_disk.sort();
        let mut served: Vec<String> = FILES
            .iter()
            .map(|f| format!("{}/{}", f.skill, f.path))
            .collect();
        served.sort();
        assert_eq!(served, on_disk, "add new skill files to FILES");
        // And every relative link in them names one of them.
        for f in FILES {
            for link in f.text().split("](").skip(1) {
                let target = link.split(')').next().unwrap();
                if target.starts_with("http") || target.starts_with('#') {
                    continue;
                }
                let dir = std::path::Path::new(f.path).parent().unwrap();
                let path = dir.join(target).to_string_lossy().replace('\\', "/");
                let uri = format!("{URI_PREFIX}{}/{path}", f.skill);
                assert!(
                    FILES.iter().any(|g| g.uri() == uri),
                    "{}/{}: broken link {target}",
                    f.skill,
                    f.path
                );
            }
        }
    }

    #[test]
    fn the_guide_comes_with_the_safety_rules() {
        let got = prompts_get(&json!({"name": "computer-use"})).unwrap();
        let messages = got["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        let text = |i: usize| messages[i]["content"]["text"].as_str().unwrap();
        assert!(text(0).starts_with("# Computer use\n"), "{}", text(0));
        assert!(text(1).starts_with("# Computer use: security rules"));
    }

    #[test]
    fn errors_use_the_spec_codes() {
        assert_eq!(prompts_get(&json!({})).unwrap_err().0, INVALID_PARAMS);
        assert_eq!(
            prompts_get(&json!({"name": "nope"})).unwrap_err().0,
            INVALID_PARAMS
        );
        for uri in [
            "computer-use://skills/nope/SKILL.md",
            "computer-use://skills/computer-use/../../Cargo.toml",
            "file:///etc/passwd",
        ] {
            assert_eq!(
                resources_read(&json!({ "uri": uri })).unwrap_err().0,
                RESOURCE_NOT_FOUND,
                "{uri}"
            );
        }
        assert!(handle("tools/list", &json!({})).is_none());
    }

    #[test]
    fn crlf_checkouts_are_served_with_lf() {
        let f = File {
            skill: "x",
            path: "SKILL.md",
            raw: "---\r\nname: x\r\ndescription: D\r\n---\r\n# T\r\nbody\r\n",
        };
        assert_eq!(f.text(), "---\nname: x\ndescription: D\n---\n# T\nbody\n");
        assert_eq!(f.body(), "# T\nbody\n");
        assert_eq!(f.description(), "D");
    }

    #[test]
    fn front_matter_is_parsed() {
        let (meta, body) =
            front_matter("---\nname: x\ndescription: >-\n  One\n  two.\n---\n\n# T\n");
        assert_eq!(description(meta).as_deref(), Some("One two."));
        assert_eq!(title(body), "T");
        assert_eq!(front_matter("# Plain\n"), ("", "# Plain\n"));
        let (meta, body) = front_matter("---\r\ndescription: >-\r\n  A\r\n---\r\n# T\r\n");
        assert_eq!(description(meta).as_deref(), Some("A"));
        assert_eq!(title(body), "T");
        assert_eq!(
            description("description: One line").as_deref(),
            Some("One line")
        );
    }
}
