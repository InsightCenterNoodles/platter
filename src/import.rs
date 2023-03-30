use std::{path::Path, fmt::Display};

use crate::intermediate::*;

#[derive(Debug)]
pub enum ImportError {
    UnableToOpenFile(String),
    UnableToImport(String),
}

impl Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for ImportError {
    
}

pub fn import_file(path: &Path) -> Result<IntermediateScene, ImportError> {
    Err(ImportError::UnableToImport("TEST".to_string()))
}