use std::path::Path;

use gltf;
use crate::{intermediate::*, import::ImportError};

impl From<gltf::Error> for ImportError {
    fn from(value: gltf::Error) -> Self {
        ImportError::UnableToImport(value.to_string())
    }
}

pub fn import_file(path: &Path) -> Result<IntermediateScene, ImportError> {

    let (gltf, buffers, images) = gltf::import(path)?;

    for buffer in buffers {
        Inter buffer.0
    }

    for node in gltf.nodes() {
        node.mesh()?.primitives()?.
    }

    Ok(IntermediateScene::default())
}