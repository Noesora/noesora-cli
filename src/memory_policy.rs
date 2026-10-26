use serde::{Deserialize, Serialize};

/// Authorization for the connection Noesora supplies, not a native filesystem sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "project", rename_all = "kebab-case")]
pub enum MemoryPolicy {
    Owner,
    ProjectReadOnly(String),
    AllReadOnly,
}

impl MemoryPolicy {
    pub fn project(project: String) -> Result<Self, String> {
        if project.trim().is_empty() {
            return Err(
                "managed memory requires an exact nonempty persisted project identity".into(),
            );
        }
        Ok(Self::ProjectReadOnly(project))
    }

    pub fn managed(&self) -> bool {
        !matches!(self, Self::Owner)
    }
    pub fn owner_tools(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn filter_project(&self, requested: Option<&str>) -> Result<Option<String>, String> {
        match self {
            Self::ProjectReadOnly(project) => {
                if requested.is_some_and(|value| value != project) {
                    return Err(
                        "requested project is outside this managed memory connection".into(),
                    );
                }
                Ok(Some(project.clone()))
            }
            Self::Owner | Self::AllReadOnly => Ok(requested.map(str::to_owned)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_scope_cannot_be_widened() {
        let policy = MemoryPolicy::project("project-a".into()).unwrap();
        assert_eq!(
            policy.filter_project(None).unwrap().as_deref(),
            Some("project-a")
        );
        assert!(policy.filter_project(Some("project-b")).is_err());
        assert!(!policy.owner_tools());
        assert!(MemoryPolicy::project("  ".into()).is_err());
    }

    #[test]
    fn all_projects_scope_is_read_only_and_filters_only_narrow() {
        let policy = MemoryPolicy::AllReadOnly;
        assert!(policy.managed());
        assert!(!policy.owner_tools());
        assert_eq!(policy.filter_project(None).unwrap(), None);
        assert_eq!(
            policy.filter_project(Some("project-a")).unwrap().as_deref(),
            Some("project-a")
        );
    }

    #[test]
    fn owner_policy_retains_owner_tools() {
        assert!(!MemoryPolicy::Owner.managed());
        assert!(MemoryPolicy::Owner.owner_tools());
    }
}
