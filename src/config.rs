//! Global Noesora store selection. The engine resolves NOESORA_HOME or ~/.noesora.

use noesora_engine::{EngineResult, Store};

/// Ensure store-root paths in JSON responses can be represented losslessly.
pub(crate) fn require_json_safe_store(store: &Store) -> EngineResult<()> {
    if store.root().to_str().is_some() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "store root path is not valid UTF-8 for JSON output",
        )
        .into())
    }
}

/// Open the one user-global store; the process working directory is never consulted.
pub fn open_store() -> EngineResult<Store> {
    Store::from_env()
}
