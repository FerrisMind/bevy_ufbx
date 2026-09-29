//! Contract tests for the L-MESH-ERR leaf (plan leaves D6, H-MESH, O5):
//!
//! - **D6** — `JOINT_INDEX`/`JOINT_WEIGHT` are written only on meshes a skin
//!   instance actually references (`skin_instanced_mesh_elements`), mirroring
//!   `bevy_gltf`'s joints/weights guard (`loader/mod.rs:759-770`);
//! - **H-MESH** — `FbxPrimitive.name` is populated in the
//!   `"{mesh}.{material}"` glTF-parity form for every primitive;
//! - **O5** — `FbxError` renders actionable messages and is
//!   `#[non_exhaustive]` (this file is a downstream crate, so a match on it
//!   needs the wildcard arm).

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::image::Image;
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::mesh::skin_instanced_mesh_elements;
use bevy_ufbx::{Fbx, FbxError, FbxMesh, FbxPlugin};

// ── helpers ─────────────────────────────────────────────────────────────────

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: "assets".into(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .init_asset::<AnimationClip>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .init_asset::<WorldAsset>();
    app
}

fn wait_for_load(app: &mut App, handle: &Handle<Fbx>, max_frames: usize) -> bool {
    for _ in 0..max_frames {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.load_state(handle) {
            LoadState::Loaded => return true,
            LoadState::Failed(_) => return false,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    false
}

fn load_fbx(app: &mut App, path: &str) -> Handle<Fbx> {
    let handle: Handle<Fbx> = app.world().resource::<AssetServer>().load(path.to_string());
    assert!(wait_for_load(app, &handle, 500), "{path} failed to load");
    handle
}

/// ufbx-side prediction of which meshes a skin instance references, computed
/// independently of the asset loader.
fn predicted_skin_instances(path: &str) -> std::collections::HashSet<u32> {
    let bytes = std::fs::read(format!("assets/{path}")).expect("fixture bytes");
    let root = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("ufbx load");
    let scene: &ufbx::Scene = &root;
    skin_instanced_mesh_elements(scene)
}

// ── D6: joints/weights guard ────────────────────────────────────────────────

/// Non-skinned fixtures must lose `JOINT_INDEX`/`JOINT_WEIGHT` entirely —
/// `bevy_gltf` only keeps joints/weights on primitives used by skinned nodes.
#[test]
fn non_skinned_fixture_primitives_carry_no_joint_attributes() {
    for fixture in ["cube.fbx", "blender_suzanne_multimaterial_7400_binary.fbx"] {
        let mut app = headless_app();
        let fbx_handle = load_fbx(&mut app, fixture);

        let primitive_handles = {
            let fbxs = app.world().resource::<Assets<Fbx>>();
            let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
            assert!(
                !fbx.primitive_meshes.is_empty(),
                "{fixture} must have primitives"
            );
            fbx.primitive_meshes.clone()
        };

        let meshes = app.world().resource::<Assets<Mesh>>();
        for handle in &primitive_handles {
            let mesh = meshes.get(handle).expect("primitive mesh");
            assert!(
                mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).is_none(),
                "{fixture}: non-skinned primitive must not carry JOINT_INDEX"
            );
            assert!(
                mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).is_none(),
                "{fixture}: non-skinned primitive must not carry JOINT_WEIGHT"
            );
        }
    }
}

/// The skinned fixture keeps both attributes exactly as before (values and
/// `MAX_JOINTS` semantics are covered by `skin_loading`).
#[test]
fn skinned_fixture_primitives_keep_joint_attributes() {
    let mut app = headless_app();
    let fbx_handle = load_fbx(&mut app, "rigged_triangle.fbx");

    let mesh_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.primitive_meshes[0].clone()
    };

    let meshes = app.world().resource::<Assets<Mesh>>();
    let mesh = meshes.get(&mesh_handle).expect("primitive mesh");
    let Some(VertexAttributeValues::Uint16x4(indices)) =
        mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
    else {
        panic!("skinned primitive must keep JOINT_INDEX");
    };
    assert!(!indices.is_empty());
    assert!(
        mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).is_some(),
        "skinned primitive must keep JOINT_WEIGHT"
    );
}

/// The guard's decision function must agree with the skins the loader actually
/// emits (`Fbx::skins`): predicted set empty ⇔ no `FbxSkin` assets.
#[test]
fn skin_instance_prediction_agrees_with_emitted_skins() {
    for fixture in [
        "cube.fbx",
        "blend_shape_cube.fbx",
        "nurbs_saddle.fbx",
        "blender_suzanne_multimaterial_7400_binary.fbx",
        "rigged_triangle.fbx",
        "blender_279_sausage_7400_binary.fbx",
    ] {
        let predicted = predicted_skin_instances(fixture);

        let mut app = headless_app();
        let fbx_handle = load_fbx(&mut app, fixture);
        let emitted_skins = {
            let fbxs = app.world().resource::<Assets<Fbx>>();
            let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
            !fbx.skins.is_empty()
        };

        assert_eq!(
            predicted.is_empty(),
            !emitted_skins,
            "{fixture}: skin-instance prediction {predicted:?} disagrees with emitted skins"
        );
    }
}

// ── H-MESH: primitive naming ────────────────────────────────────────────────

/// Every `FbxPrimitive` on the multi-material fixture is named
/// `"{mesh}.{material}"`, with names read independently through ufbx.
#[test]
fn primitive_names_populate_from_mesh_and_material() {
    const FIXTURE: &str = "blender_suzanne_multimaterial_7400_binary.fbx";
    let bytes = std::fs::read(format!("assets/{FIXTURE}")).expect("fixture bytes");

    // Expected names, read directly through ufbx (the loader's data source).
    let root = ufbx::load_memory(&bytes, ufbx::LoadOpts::default()).expect("ufbx load");
    let scene: &ufbx::Scene = &root;
    let ufbx_mesh = scene
        .nodes
        .as_ref()
        .iter()
        .find_map(|node| node.mesh.as_ref().map(|mesh| mesh.as_ref()))
        .expect("fixture mesh");
    let mesh_name: &str = ufbx_mesh.element.name.as_ref();
    let material_names: Vec<&str> = ufbx_mesh
        .materials
        .as_ref()
        .iter()
        .map(|material| material.element.name.as_ref())
        .collect();
    assert!(
        material_names.len() >= 2,
        "fixture must be multi-material, got {material_names:?}"
    );

    let mut app = headless_app();
    let fbx_handle = load_fbx(&mut app, FIXTURE);
    let fbx_mesh_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.meshes.first().expect("FbxMesh handle").clone()
    };

    let fbx_meshes = app.world().resource::<Assets<FbxMesh>>();
    let fbx_mesh = fbx_meshes.get(&fbx_mesh_handle).expect("FbxMesh asset");
    assert!(
        fbx_mesh.primitives.len() >= 2,
        "multi-material fixture must split into primitives"
    );

    for primitive in &fbx_mesh.primitives {
        let name = primitive
            .name
            .as_deref()
            .expect("H-MESH: FbxPrimitive.name must be populated");
        let material = name
            .strip_prefix(mesh_name)
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or_else(|| panic!("primitive name {name:?} must be {mesh_name}.<material>"));
        assert!(
            material_names.contains(&material),
            "primitive name {name:?} references unknown material {material:?}"
        );
    }

    // The fixture exercises the joined form, not just the mesh-only fallback.
    assert!(
        fbx_mesh
            .primitives
            .iter()
            .any(|primitive| primitive.name.as_deref().is_some_and(|n| n.contains('.'))),
        "expected at least one joined {{mesh}}.{{material}}-style name"
    );
}

// ── O5: granular FbxError ───────────────────────────────────────────────────

/// Structured variants carry path / element / underlying message; every
/// variant renders something actionable.
#[test]
fn fbx_error_variants_render_actionable_messages() {
    let parse = FbxError::UfbxLoad {
        path: "hero.fbx".to_string(),
        message: "BadMagic".to_string(),
    };
    let message = parse.to_string();
    assert!(
        message.contains("hero.fbx") && message.contains("BadMagic"),
        "unactionable parse error: {message}"
    );

    let morph = FbxError::MorphTargets {
        mesh: "Suzanne".to_string(),
        message: "morph targets: vertex count mismatch".to_string(),
    };
    let message = morph.to_string();
    assert!(
        message.contains("Suzanne") && message.contains("vertex count mismatch"),
        "unactionable morph error: {message}"
    );

    let invalid = FbxError::InvalidData("Empty FBX file".to_string());
    assert!(invalid.to_string().contains("Empty FBX file"));

    let io = FbxError::Io(std::io::Error::other("permission denied"));
    assert!(io.to_string().contains("permission denied"));

    // String-fallback variants keep their prefixes and inner context.
    for (error, needle) in [
        (FbxError::UfbxError("BadMagic".to_string()), "BadMagic"),
        (
            FbxError::ConversionError("curve out of range".to_string()),
            "curve out of range",
        ),
        (
            FbxError::MeshConversion("degenerate triangle".to_string()),
            "degenerate triangle",
        ),
        (
            FbxError::MaterialConversion("missing shader".to_string()),
            "missing shader",
        ),
        (
            FbxError::TextureLoad("diffuse.png".to_string()),
            "diffuse.png",
        ),
        (
            FbxError::UnsupportedFeature("dual-quaternion skinning".to_string()),
            "dual-quaternion",
        ),
    ] {
        let message = error.to_string();
        assert!(message.contains(needle), "unactionable message: {message}");
    }
}

/// `FbxError` is `#[non_exhaustive]`: this file is a downstream crate, so a
/// match without a wildcard arm would not compile. Compiling *with* the `_`
/// arm is the contract.
#[test]
fn fbx_error_match_downstream_requires_wildcard_arm() {
    fn classify(error: &FbxError) -> &'static str {
        match error {
            FbxError::Io(_) => "io",
            FbxError::UfbxLoad { .. } => "parse",
            FbxError::InvalidData(_) => "invalid-data",
            FbxError::MorphTargets { .. } => "morph",
            // Required by `#[non_exhaustive]` in every downstream crate.
            _ => "other",
        }
    }

    assert_eq!(classify(&FbxError::Io(std::io::Error::other("x"))), "io");
    assert_eq!(
        classify(&FbxError::UfbxLoad {
            path: "a.fbx".to_string(),
            message: "b".to_string(),
        }),
        "parse"
    );
    assert_eq!(
        classify(&FbxError::InvalidData("Empty FBX file".to_string())),
        "invalid-data"
    );
    assert_eq!(
        classify(&FbxError::MorphTargets {
            mesh: "Cube".to_string(),
            message: "m".to_string(),
        }),
        "morph"
    );
    assert_eq!(
        classify(&FbxError::UnsupportedFeature("x".to_string())),
        "other"
    );
}
