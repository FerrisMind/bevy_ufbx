//! Tangent handedness: pure [`tangent_sign`] units, plus real-loader tangent `w`
//! values on the corpus fixture `assets/blender340_tangent_sign_7400_binary.fbx`.
//!
//! That fixture is copied verbatim from `libs/ufbx/data/blender340_tangent_sign_7400_binary.fbx`
//! and contains four quads (`Positive`, `FlipX`, `FlipY`, `FlipXY`) whose FBX
//! bitangents deliberately flip handedness — so `w = 1.0` everywhere is provably wrong.

use std::collections::HashSet;
use std::time::Duration;

use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::mesh::{group_faces_by_material, tangent_sign};
use bevy_ufbx::{Fbx, FbxPlugin};

const TANGENT_FIXTURE: &str = "blender340_tangent_sign_7400_binary.fbx";

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

#[test]
fn tangent_sign_matches_bevy_bitangent_convention() {
    // Bevy reconstructs the bitangent as `w * cross(N, T)`
    // (`bevy_pbr` `calculate_tbn_mikktspace`), the glTF convention.
    assert_eq!(tangent_sign(Vec3::Z, Vec3::X, Vec3::Y), 1.0);
    assert_eq!(tangent_sign(Vec3::Z, Vec3::X, Vec3::NEG_Y), -1.0);
    assert_eq!(tangent_sign(Vec3::X, Vec3::Z, Vec3::NEG_Y), 1.0);
}

#[test]
fn tangent_sign_is_scale_invariant() {
    assert_eq!(
        tangent_sign(Vec3::Z * 2.0, Vec3::X * 3.0, Vec3::NEG_Y * 5.0),
        -1.0
    );
    assert_eq!(
        tangent_sign(Vec3::Z * 0.01, Vec3::X * 0.01, Vec3::Y * 0.01),
        1.0
    );
}

#[test]
fn tangent_sign_falls_back_when_frame_is_degenerate() {
    const FALLBACK: f32 = 1.0;
    // Absent / zero-length bitangent, tangent, or normal.
    assert_eq!(tangent_sign(Vec3::Z, Vec3::X, Vec3::ZERO), FALLBACK);
    assert_eq!(tangent_sign(Vec3::Z, Vec3::ZERO, Vec3::Y), FALLBACK);
    assert_eq!(tangent_sign(Vec3::ZERO, Vec3::X, Vec3::Y), FALLBACK);
    // Non-finite inputs.
    assert_eq!(
        tangent_sign(Vec3::Z, Vec3::X, Vec3::new(f32::NAN, 0.0, 0.0)),
        FALLBACK
    );
    assert_eq!(
        tangent_sign(Vec3::Z, Vec3::X, Vec3::splat(f32::INFINITY)),
        FALLBACK
    );
    // Tangent parallel to the normal, or bitangent orthogonal to the frame.
    assert_eq!(tangent_sign(Vec3::Z, Vec3::Z, Vec3::Y), FALLBACK);
    assert_eq!(tangent_sign(Vec3::Z, Vec3::X, Vec3::X), FALLBACK);
}

/// Tangent `w` counts over every primitive mesh of a loaded FBX.
fn loaded_w_counts(app: &App, fbx: &Handle<Fbx>) -> (usize, usize, usize) {
    let meshes = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(fbx).expect("Fbx asset");
        fbx.primitive_meshes.clone()
    };
    let (mut plus, mut minus, mut primitives) = (0usize, 0usize, 0usize);
    for mesh_handle in &meshes {
        let mesh_assets = app.world().resource::<Assets<Mesh>>();
        let Some(mesh) = mesh_assets.get(mesh_handle) else {
            continue;
        };
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            continue;
        };
        primitives += 1;
        assert_eq!(tangents.len(), mesh.count_vertices());
        for tangent in tangents {
            let w = tangent[3];
            assert!(
                w == 1.0 || w == -1.0,
                "tangent w must be ±1 (Bevy handedness), got {w}"
            );
            if w > 0.0 {
                plus += 1;
            } else {
                minus += 1;
            }
        }
    }
    (plus, minus, primitives)
}

/// Ground-truth `w` counts straight from ufbx, mirroring the loader's default
/// conversion path for a Blender binary file (`FbxSpaceConversion::Auto`).
fn ufbx_w_counts(bytes: &[u8]) -> (usize, usize) {
    let root = ufbx::load_memory(
        bytes,
        ufbx::LoadOpts {
            target_unit_meters: 1.0,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_camera_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_light_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            space_conversion: ufbx::SpaceConversion::AdjustTransforms,
            geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
            inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
            generate_missing_normals: true,
            use_blender_pbr_material: true,
            ..Default::default()
        },
    )
    .expect("ufbx load");
    let scene: &ufbx::Scene = &root;

    let mut seen_meshes: HashSet<u32> = HashSet::new();
    let (mut plus, mut minus) = (0usize, 0usize);
    for node in scene.nodes.as_ref().iter() {
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh = mesh_ref.as_ref();
        if !seen_meshes.insert(mesh.element.element_id) || !mesh.vertex_tangent.exists {
            continue;
        }
        for corners in group_faces_by_material(mesh).into_values() {
            for corner in corners {
                let corner = corner as usize;
                let t = mesh.vertex_tangent[corner];
                let tangent = Vec3::new(t.x as f32, t.y as f32, t.z as f32);
                let w = if mesh.vertex_bitangent.exists && mesh.vertex_normal.exists {
                    let n = mesh.vertex_normal[corner];
                    let b = mesh.vertex_bitangent[corner];
                    tangent_sign(
                        Vec3::new(n.x as f32, n.y as f32, n.z as f32),
                        tangent,
                        Vec3::new(b.x as f32, b.y as f32, b.z as f32),
                    )
                } else {
                    1.0
                };
                if w > 0.0 {
                    plus += 1;
                } else {
                    minus += 1;
                }
            }
        }
    }
    (plus, minus)
}

#[test]
fn corpus_tangent_sign_fixture_uses_bitangent_handedness() {
    let mut app = headless_app();
    let handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(TANGENT_FIXTURE)
    };
    assert!(
        wait_for_load(&mut app, &handle, 500),
        "{TANGENT_FIXTURE} failed to load"
    );

    let (plus, minus, primitives) = loaded_w_counts(&app, &handle);
    assert_eq!(
        primitives, 4,
        "fixture should load four quad primitives (Positive/FlipX/FlipY/FlipXY)"
    );
    assert_eq!(plus, 12, "Positive/FlipXY quads keep w = +1");
    assert_eq!(
        minus, 12,
        "FlipX/FlipY quads need w = -1; a hardcoded +1 regresses this"
    );
}

#[test]
fn corpus_tangent_signs_match_ufbx_bitangent_formula() {
    let bytes = std::fs::read(format!("assets/{TANGENT_FIXTURE}")).expect("fixture bytes");
    let mut app = headless_app();
    let handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(TANGENT_FIXTURE)
    };
    assert!(
        wait_for_load(&mut app, &handle, 500),
        "{TANGENT_FIXTURE} failed to load"
    );

    let (loaded_plus, loaded_minus, _) = loaded_w_counts(&app, &handle);
    let (expected_plus, expected_minus) = ufbx_w_counts(&bytes);
    assert_eq!(
        (loaded_plus, loaded_minus),
        (expected_plus, expected_minus),
        "loader must derive w from the FBX bitangent exactly like sign(dot(cross(N, T), B))"
    );
}
