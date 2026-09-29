use thiserror::Error;

/// Why loading an FBX asset failed.
///
/// Granularity modeled on `bevy_gltf`'s `GltfError`
/// (`references/bevy_gltf/src/loader/mod.rs`): one variant per failure kind,
/// with structured variants carrying the context needed to act on the failure
/// — asset path, element name, or the underlying ufbx message. The original
/// string-tuple variants are kept so existing construction sites remain
/// source-compatible; new call sites should prefer the structured variants
/// when context is cheap to pass.
///
/// The enum is `#[non_exhaustive]`: matches outside this crate need a `_` arm,
/// so further variants can be added without breaking downstream users.
///
/// Deliberate policy (parity matrix O5): a texture *decode* failure never
/// becomes an [`FbxError`] — the loader warns and falls back to the external
/// image reference instead of failing the load (a documented superset of
/// `bevy_gltf`, which fails the whole load for image errors).
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum FbxError {
    /// The FBX file (or a file it references) could not be read.
    #[error("Failed to read FBX file: {0}")]
    Io(#[from] std::io::Error),

    /// ufbx failed to open or parse the document, without asset-path context.
    #[error("Failed to load FBX: {0}")]
    UfbxError(String),

    /// ufbx failed to open or parse the document; carries the asset path so
    /// the failing file is identifiable in a multi-asset load.
    #[error("Failed to load FBX `{path}`: {message}")]
    UfbxLoad {
        /// Path of the asset being loaded.
        path: String,
        /// Debug rendering of the underlying `ufbx` error.
        message: String,
    },

    /// The file bytes failed validation before parsing (empty or truncated).
    #[error("Invalid FBX data: {0}")]
    InvalidData(String),

    /// Generic FBX → Bevy conversion failure (no single element to blame).
    #[error("Failed to convert FBX data: {0}")]
    ConversionError(String),

    /// Bevy mesh conversion failed (attributes, indices, bounds, ...).
    #[error("Failed to convert mesh: {0}")]
    MeshConversion(String),

    /// Morph targets or morph weights could not be built; carries the ufbx
    /// mesh element name so the failing mesh is identifiable.
    #[error("Failed to convert morph targets on mesh `{mesh}`: {message}")]
    MorphTargets {
        /// `ufbx::Mesh` element name (empty when the mesh is unnamed).
        mesh: String,
        /// What exactly failed, from Bevy's morph build error.
        message: String,
    },

    /// Bevy material conversion failed.
    #[error("Failed to convert material: {0}")]
    MaterialConversion(String),

    /// A texture referenced by the FBX could not be loaded.
    #[error("Failed to load texture: {0}")]
    TextureLoad(String),

    /// The file uses a feature the loader deliberately does not implement.
    #[error("Unsupported FBX feature: {0}")]
    UnsupportedFeature(String),
}
