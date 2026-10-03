//! The tool manager: `find_tools`, and the arguments of a tool called wrongly.

use super::*;

impl<B: Backend> Engine<B> {
    /// The tools of a category, or whose name or description has the words
    /// asked for, with their arguments. With [tools] manager =
    /// "list_changed", their categories join the tool list for good.
    pub(super) fn find_tools(&mut self, args: &serde_json::Value) -> ToolOutput {
        use crate::config::ToolManager;
        /// Tools a query returns at most (a category or names return all).
        const MOST: usize = 3;
        let category = args.get("category").and_then(serde_json::Value::as_str);
        let again = args
            .get("again")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let names: Vec<String> = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|n| {
                n.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|w| !w.is_empty())
                    .map(|w| w.to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let words = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(crate::tools::query_words)
            .unwrap_or_default();
        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let all = self.all_tool_definitions();
        let hidden = |d: &&ToolDefinition| !crate::tools::BASE_TOOLS.contains(&&*d.name);
        let in_category =
            |d: &&ToolDefinition| category.is_none_or(|c| crate::tools::category_of(&d.name) == c);
        let mut more = Vec::new();
        // How well the best tool matched the words (3: a word in its name).
        let mut best = 0;
        let mut found: Vec<&ToolDefinition> = if !names.is_empty() {
            all.iter()
                .filter(|d| names.contains(&d.name.to_string()))
                .collect()
        } else if words.is_empty() {
            // A category on its own: all of it.
            all.iter()
                .filter(hidden)
                .filter(in_category)
                .filter(|_| category.is_some())
                .collect()
        } else {
            // By what they do: the best few, words in the name counting most.
            let mut scored: Vec<(usize, &ToolDefinition)> = all
                .iter()
                .filter(hidden)
                .filter(in_category)
                .map(|d| {
                    (
                        crate::tools::query_score(&words, &d.name, &d.description),
                        d,
                    )
                })
                .filter(|(score, _)| *score > 0)
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            best = scored.first().map_or(0, |s| s.0);
            more = scored
                .iter()
                .skip(MOST)
                .map(|(_, d)| d.name.to_string())
                .collect();
            scored.into_iter().take(MOST).map(|(_, d)| d).collect()
        };
        // Words that name no tool, or only show up in descriptions: the
        // decision model, when there is one, knows "make a logo" means
        // design. Without one the words decide, as before.
        let mut by_model = false;
        if names.is_empty()
            && let Some(q) = query
            && best < 3
        {
            let cands: Vec<&ToolDefinition> =
                all.iter().filter(hidden).filter(in_category).collect();
            if let Some(picked) = self.tools_by_model(q, &cands) {
                found = picked.into_iter().map(|i| cands[i]).collect();
                more.clear();
                by_model = true;
            }
        }
        if found.is_empty() {
            let cats: Vec<&str> = crate::tools::CATEGORIES.iter().map(|c| c.0).collect();
            return ToolOutput::text(format!("No tools match. Categories: {}.", cats.join(", ")));
        }
        let dispatch = self.store.config.tools.manager == ToolManager::Dispatch;
        let mut out = String::new();
        for d in &found {
            out.push_str(&format!("{}: {}\n", d.name, d.description));
            // With list_changed the arguments come with the tool list.
            if !dispatch {
                continue;
            }
            let schema = d.input_schema.to_string();
            let hash = text_hash(&schema);
            if !again && self.schemas_shown.get(&*d.name) == Some(&hash) {
                out.push_str("  arguments: as shown before (again=true repeats them)\n");
            } else {
                out.push_str(&format!("  arguments: {schema}\n"));
                self.schemas_shown.insert(d.name.to_string(), hash);
            }
        }
        if !more.is_empty() {
            out.push_str(&format!("Also matching: {}.\n", more.join(", ")));
        }
        if by_model {
            out.push_str("(Chosen by the decision model.)\n");
        }
        if dispatch {
            out.push_str("Run one with use_tool(name, arguments).");
        } else {
            // Base tools are listed already: no category joins for them.
            let added: Vec<&str> = found
                .iter()
                .copied()
                .filter(hidden)
                .map(|d| &*d.name)
                .collect();
            for name in &added {
                self.active_tools.insert(crate::tools::category_of(name));
            }
            if added.is_empty() {
                out.push_str("Already in your tools (call them directly).");
            } else {
                out.push_str(&format!(
                    "Added to your tools: {} (call them directly).",
                    added.join(", ")
                ));
            }
        }
        ToolOutput::text(out)
    }

    /// A tool the model hasn't been shown was called with arguments it
    /// doesn't take: its arguments, once, so the next call can be right
    /// without a find_tools first.
    pub(super) fn schema_for_wrong_call(&mut self, name: &str) -> Option<String> {
        if self.store.config.tools.manager == crate::config::ToolManager::Off
            || self.tool_definitions().iter().any(|d| d.name == name)
        {
            return None;
        }
        let d = self
            .all_tool_definitions()
            .into_iter()
            .find(|d| d.name == name)?;
        let schema = d.input_schema.to_string();
        let hash = text_hash(&schema);
        if self.schemas_shown.get(name) == Some(&hash) {
            return None;
        }
        self.schemas_shown.insert(name.to_string(), hash);
        Some(format!("\n{name} takes: {schema}"))
    }
}
