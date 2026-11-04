//! Error type returned by the public API.
//!
//! Every message is written for an end user (they are shown verbatim by the CLI and can be shown
//! verbatim in a GUI). `kind()` gives a stable machine-readable tag for GUIs that want to react
//! differently (e.g. highlight the input field for `input_not_found`).

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("Could not find the file:\n    {}\n\nDouble-check the spelling and the folder.", .0.display())]
    InputNotFound(PathBuf),

    #[error("This converter only reads .glb 3D model files, but you gave it a {0} file.\n\n\
             If you downloaded the model from Sketchfab or a similar site, look for a \".glb\" download option.\n\
             If you exported it through Blender, export as \"glTF 2.0 (.glb/.gltf)\".")]
    NotGlb(String),

    #[error("This file could not be read as a 3D model ({0}).\n\n\
             The file is likely damaged, incomplete, or not really a .glb file despite its name.\n\n\
             Try downloading/exporting it again.")]
    Unreadable(String),

    #[error("This file loaded, but it does not contain any visible 3D shapes (no faces/triangles were found).\n\
             It may be an empty scene, a file that only holds an animation/skeleton, or a file that saved wrong.\n\
             Try re-exporting or re-downloading it.")]
    NoGeometry,

    #[error("The \"drop transparent materials\" option removed every single part of this model, because every material\n\
             in it is non-opaque (glass, transparent plastic, etc). Try again with that option off.")]
    EverythingTransparent,

    #[error("A folder called \"{name}\" already exists:\n    {}\n\n\
             It wasn't created by this converter, so the process was cancelled in case \n\
             this was a mistake.\n\nRename, delete or move that folder, or run this again\n\
             with a different output name.", .path.display())]
    OutputFolderNotOurs { name: String, path: PathBuf },

    #[error("A folder called \"{name}\" already exists:\n    {}\n\nThe overwrite policy is set to fail, so nothing was changed.", .path.display())]
    OutputExists { name: String, path: PathBuf },

    #[error("Your system won't let the converter write to:\n    {}\n\n\
             This usually means a file inside that folder is open in another program (close it and\n\
             try again), or the folder is somewhere protected like Program Files.\n\n\
             Try a normal folder such as your Desktop.", .0.display())]
    PermissionDenied(PathBuf),

    #[error("Invalid option: {0}")]
    InvalidOption(String),

    #[error("The conversion was cancelled.")]
    Cancelled,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Something unexpected went wrong: {0}")]
    Internal(String),
}

impl ConvertError {
    /// Stable machine-readable tag.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InputNotFound(_) => "input_not_found",
            Self::NotGlb(_) => "not_glb",
            Self::Unreadable(_) => "unreadable",
            Self::NoGeometry => "no_geometry",
            Self::EverythingTransparent => "everything_transparent",
            Self::OutputFolderNotOurs { .. } => "output_folder_not_ours",
            Self::OutputExists { .. } => "output_exists",
            Self::PermissionDenied(_) => "permission_denied",
            Self::InvalidOption(_) => "invalid_option",
            Self::Cancelled => "cancelled",
            Self::Io(_) => "io",
            Self::Internal(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, ConvertError>;
