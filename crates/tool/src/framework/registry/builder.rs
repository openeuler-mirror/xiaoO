use agent_contracts::tool::{ToolRegistry, ToolRegistryBuilder, ToolSource};
use agent_types::common::BuildError;
use agent_types::tool::ToolRegistryConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::ToolRegistryImpl;

pub struct ToolRegistryBuilderImpl {
    sources: Vec<Box<dyn ToolSource>>,
    config: Option<ToolRegistryConfig>,
}

impl ToolRegistryBuilderImpl {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            config: None,
        }
    }

    fn collect_discovered_tools(
        &self,
    ) -> Result<Vec<agent_contracts::tool::DiscoveredTool>, BuildError> {
        let discovered_tools: Vec<_> = self
            .sources
            .iter()
            .flat_map(|source| source.discover())
            .collect();

        self.validate_discovered_tools(&discovered_tools)?;

        Ok(discovered_tools)
    }

    fn validate_discovered_tools(
        &self,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<(), BuildError> {
        self.validate_duplicate_tool_names(discovered_tools)?;
        self.validate_discovered_tool_pairs(discovered_tools)
    }

    fn validate_duplicate_tool_names(
        &self,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<(), BuildError> {
        let mut tool_names = HashSet::new();

        for discovered_tool in discovered_tools {
            let tool_name = discovered_tool.spec.name().0.clone();
            if !tool_names.insert(tool_name.clone()) {
                return Err(BuildError::InvalidConfig {
                    message: format!("duplicate tool name in registry: {}", tool_name),
                });
            }
        }

        Ok(())
    }

    fn validate_discovered_tool_pairs(
        &self,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<(), BuildError> {
        for discovered_tool in discovered_tools {
            let executor_spec = discovered_tool.executor.spec();

            if discovered_tool.spec.id() != executor_spec.id() {
                return Err(BuildError::DependencyError {
                    message: format!(
                        "tool spec/executor id mismatch: discovered spec '{}' != executor spec '{}'",
                        discovered_tool.spec.id(),
                        executor_spec.id()
                    ),
                });
            }

            if discovered_tool.spec.name() != executor_spec.name() {
                return Err(BuildError::DependencyError {
                    message: format!(
                        "tool spec/executor name mismatch: discovered spec '{}' != executor spec '{}'",
                        discovered_tool.spec.name(),
                        executor_spec.name()
                    ),
                });
            }
        }

        Ok(())
    }

    fn build_executor_map(
        &self,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<
        std::collections::HashMap<
            agent_types::common::ids::ToolId,
            std::sync::Arc<dyn agent_contracts::tool::ToolExecutor>,
        >,
        BuildError,
    > {
        let mut executors = HashMap::new();

        for discovered_tool in discovered_tools {
            let tool_id = discovered_tool.spec.id().clone();
            if executors
                .insert(tool_id.clone(), Arc::clone(&discovered_tool.executor))
                .is_some()
            {
                return Err(BuildError::InvalidConfig {
                    message: format!("duplicate tool id in registry executor map: {}", tool_id),
                });
            }
        }

        Ok(executors)
    }

    fn build_spec_map(
        &self,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<
        std::collections::HashMap<
            agent_types::common::ids::ToolId,
            std::sync::Arc<dyn agent_contracts::tool::ToolSpecView>,
        >,
        BuildError,
    > {
        let mut specs = HashMap::new();

        for discovered_tool in discovered_tools {
            let tool_id = discovered_tool.spec.id().clone();
            if specs
                .insert(tool_id.clone(), Arc::clone(&discovered_tool.spec))
                .is_some()
            {
                return Err(BuildError::InvalidConfig {
                    message: format!("duplicate tool id in registry spec map: {}", tool_id),
                });
            }
        }

        Ok(specs)
    }

    fn build_registry_impl(&self) -> Result<ToolRegistryImpl, BuildError> {
        let config = self
            .config
            .clone()
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "config".to_string(),
            })?;
        let discovered_tools = self.collect_discovered_tools()?;
        self.validate_visibility_config(&config, &discovered_tools)?;
        let executors = self.build_executor_map(&discovered_tools)?;
        let specs = self.build_spec_map(&discovered_tools)?;

        Ok(ToolRegistryImpl::new(executors, specs, config.visibility))
    }

    fn validate_visibility_config(
        &self,
        config: &ToolRegistryConfig,
        discovered_tools: &[agent_contracts::tool::DiscoveredTool],
    ) -> Result<(), BuildError> {
        let discovered_tool_names: HashSet<_> = discovered_tools
            .iter()
            .map(|discovered_tool| discovered_tool.spec.name().0.clone())
            .collect();

        for allowed_tool_names in config.visibility.per_agent_allowed_tools.values() {
            for allowed_tool_name in allowed_tool_names {
                if !discovered_tool_names.contains(&allowed_tool_name.0) {
                    return Err(BuildError::InvalidConfig {
                        message: format!(
                            "unknown tool name in visibility config: {}",
                            allowed_tool_name
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

impl ToolRegistryBuilder for ToolRegistryBuilderImpl {
    fn with_sources(mut self, sources: Vec<Box<dyn ToolSource>>) -> Self {
        // 去重
        for source in sources {
            if !self
                .sources
                .iter()
                .any(|s| std::ptr::addr_eq(s.as_ref() as *const _, source.as_ref() as *const _))
            {
                self.sources.push(source);
            }
        }
        self
    }

    fn with_config(mut self, config: ToolRegistryConfig) -> Self {
        self.config = Some(config);
        self
    }

    fn build(self) -> Result<Box<dyn ToolRegistry>, BuildError> {
        Ok(Box::new(self.build_registry_impl()?))
    }
}
