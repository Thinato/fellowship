use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Pm,
    Orchestrator,
    Architect,
    Recon,
    /// Used dynamically once the engineer pool spawns in Phase 9.
    #[allow(dead_code)]
    Engineer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Pm => "pm",
            Role::Orchestrator => "orchestrator",
            Role::Architect => "architect",
            Role::Recon => "recon",
            Role::Engineer => "engineer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemberId {
    pub role: Role,
    pub instance: u32,
}

impl MemberId {
    pub fn singleton(role: Role) -> Self {
        Self { role, instance: 0 }
    }

    /// Used by the dynamic engineer pool in Phase 9.
    #[allow(dead_code)]
    pub fn engineer(instance: u32) -> Self {
        Self {
            role: Role::Engineer,
            instance,
        }
    }

    /// Stable display label (e.g. "pm", "engineer-1"). Singleton roles drop the suffix.
    pub fn label(&self) -> String {
        match self.role {
            Role::Engineer => format!("engineer-{}", self.instance),
            _ => self.role.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Surface {
    Workspace(PathBuf),
    #[allow(dead_code)] // Phase 2: variant exists; populated by Phase 3.
    Member(MemberId),
}

impl Surface {
    #[allow(dead_code)]
    pub fn workspace_path(&self) -> Option<&std::path::Path> {
        match self {
            Surface::Workspace(p) => Some(p.as_path()),
            Surface::Member(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn workspace_and_member_distinct_hashmap_keys() {
        let mut m: HashMap<Surface, u32> = HashMap::new();
        m.insert(Surface::Workspace(PathBuf::from("/a")), 1);
        m.insert(Surface::Member(MemberId::singleton(Role::Pm)), 2);
        m.insert(Surface::Member(MemberId::engineer(1)), 3);
        m.insert(Surface::Member(MemberId::engineer(2)), 4);
        assert_eq!(m.len(), 4);
        assert_eq!(m[&Surface::Workspace(PathBuf::from("/a"))], 1);
        assert_eq!(m[&Surface::Member(MemberId::engineer(1))], 3);
    }

    #[test]
    fn member_id_singleton_label_drops_instance() {
        assert_eq!(MemberId::singleton(Role::Pm).label(), "pm");
        assert_eq!(
            MemberId::singleton(Role::Orchestrator).label(),
            "orchestrator"
        );
    }

    #[test]
    fn member_id_engineer_label_includes_instance() {
        assert_eq!(MemberId::engineer(1).label(), "engineer-1");
        assert_eq!(MemberId::engineer(42).label(), "engineer-42");
    }

    #[test]
    fn workspace_path_returns_path_for_workspace_only() {
        let s = Surface::Workspace(PathBuf::from("/repo"));
        assert_eq!(s.workspace_path(), Some(Path::new("/repo")));

        let m = Surface::Member(MemberId::singleton(Role::Architect));
        assert_eq!(m.workspace_path(), None);
    }
}
