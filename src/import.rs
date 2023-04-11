use std::{fmt::Display, path::Path};

use anyhow::Result;

use colabrodo_server::{server_http::AssetStorePtr, server_state::ServerStatePtr};

use crate::object::ObjectRoot;

#[derive(Debug)]
pub enum ImportError {
    UnableToOpenFile(String),
    UnknownFileFormat(String),
    UnableToImport(String),
}

impl Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for ImportError {}

/// Attempt to import a geometry file.
pub fn import_file(
    path: &Path,
    state: ServerStatePtr,
    asset_store: AssetStorePtr,
) -> Result<ObjectRoot> {
    let ext = path.extension().and_then(|f| f.to_str()).ok_or_else(|| {
        ImportError::UnknownFileFormat(format!(
            "Unable to determine extension from path: {}",
            path.display()
        ))
    })?;

    match ext {
        "gltf" | "glb" => crate::import_gltf::import_file(path, state, asset_store),
        "obj" => crate::import_obj::import_file(path, state, asset_store),
        _ => Err(ImportError::UnknownFileFormat(format!(
            "File {} does not have a known extension",
            path.display()
        ))
        .into()),
    }
}
