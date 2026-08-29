use super::*;

fn normalized_query(q: &str) -> NormalizedQuery {
    crate::core::McpToolInput {
        q: q.to_string(),
        ..Default::default()
    }
    .normalize()
    .expect("valid test query")
}

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_root(label: &str) -> TestRoot {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    TestRoot::new(std::env::temp_dir().join(format!("sippion-{label}-{nonce}")))
}

mod access_policy;
mod filesystem_security;
mod index_properties;
mod ranking_mapping;
mod redaction_core;
mod redaction_extended;
mod resource_policy;
mod retrieval_mapping;
