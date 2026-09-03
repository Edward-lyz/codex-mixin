use super::*;

#[derive(Clone, Debug, Default)]
pub struct ToolNameMap {
    upstream_to_codex: HashMap<String, CodexToolName>,
    custom_tools: HashSet<String>,
    tool_search_execution: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodexToolName {
    name: String,
    namespace: Option<String>,
}

impl ToolNameMap {
    pub fn insert(
        &mut self,
        upstream_name: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert_mapping(
            upstream_name,
            CodexToolName {
                name: codex_name,
                namespace: None,
            },
        )
    }

    pub fn insert_namespaced(
        &mut self,
        upstream_name: String,
        namespace: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert_mapping(
            upstream_name,
            CodexToolName {
                name: codex_name,
                namespace: Some(namespace),
            },
        )
    }

    fn insert_mapping(
        &mut self,
        upstream_name: String,
        codex_name: CodexToolName,
    ) -> Result<(), GatewayError> {
        if self.upstream_to_codex.contains_key(&upstream_name) {
            return Err(GatewayError::BadRequest(format!(
                "tool names collide after upstream sanitization: {upstream_name}"
            )));
        }
        self.upstream_to_codex.insert(upstream_name, codex_name);
        Ok(())
    }

    pub fn to_codex_name<'a>(&'a self, upstream_name: &'a str) -> &'a str {
        self.upstream_to_codex
            .get(upstream_name)
            .map(|name| name.name.as_str())
            .unwrap_or(upstream_name)
    }

    pub fn to_codex_namespace(&self, upstream_name: &str) -> Option<&str> {
        self.upstream_to_codex
            .get(upstream_name)
            .and_then(|name| name.namespace.as_deref())
    }

    pub fn insert_custom(
        &mut self,
        upstream_name: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert(upstream_name.clone(), codex_name)?;
        self.custom_tools.insert(upstream_name.clone());
        Ok(())
    }

    pub fn is_custom(&self, upstream_name: &str) -> bool {
        self.custom_tools.contains(upstream_name)
    }

    pub fn insert_tool_search(
        &mut self,
        upstream_name: String,
        codex_name: String,
        execution: String,
    ) -> Result<(), GatewayError> {
        self.insert(upstream_name.clone(), codex_name)?;
        self.tool_search_execution
            .insert(upstream_name.clone(), execution);
        Ok(())
    }

    pub fn tool_search_execution(&self, upstream_name: &str) -> Option<&str> {
        self.tool_search_execution
            .get(upstream_name)
            .map(String::as_str)
    }
}
