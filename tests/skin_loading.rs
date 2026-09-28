//! Integration tests: skins, IBM labels, joint weights on rigged_triangle.fbx.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin, FbxSkin};

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

#[test]
fn rigged_triangle_loads_skin_and_ibm() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("rigged_triangle.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "rigged_triangle.fbx failed to load"
    );

    let (skin_handle, joint_count, ibm_handle) = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        assert!(!fbx.skins.is_empty(), "expected at least one FbxSkin");
        let skin_handle = fbx.skins[0].clone();
        let skins = app.world().resource::<Assets<FbxSkin>>();
        let skin = skins.get(&skin_handle).expect("Skin0");
        assert!(
            !skin.joint_element_ids.is_empty(),
            "skin should list joints in cluster order"
        );
        assert_eq!(
            skin.joints.len(),
            skin.joint_element_ids.len(),
            "joint handles and element ids must match"
        );
        (
            skin_handle.clone(),
            skin.joint_element_ids.len(),
            skin.inverse_bind_matrices.clone(),
        )
    };

    let _ = skin_handle;
    let ibms = app
        .world()
        .resource::<Assets<SkinnedMeshInverseBindposes>>();
    let ibm = ibms.get(&ibm_handle).expect("Skin0/InverseBindMatrices");
    assert_eq!(ibm.len(), joint_count, "IBM count must equal joint count");
}

#[test]
fn labeled_ibm_asset_resolves() {
    let mut app = headless_app();
    let ibm_handle: Handle<SkinnedMeshInverseBindposes> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FbxAssetLabel::InverseBindMatrices(0).from_asset("rigged_triangle.fbx"))
    };
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("rigged_triangle.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "rigged_triangle.fbx failed to load"
    );

    let ibms = app
        .world()
        .resource::<Assets<SkinnedMeshInverseBindposes>>();
    assert!(
        ibms.get(&ibm_handle).is_some(),
        "Skin0/InverseBindMatrices label should resolve"
    );
}

#[test]
fn skinned_mesh_has_top4_joint_attributes() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("rigged_triangle.fbx")
    };

    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "rigged_triangle.fbx failed to load"
    );

    let mesh_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx");
        assert!(!fbx.primitive_meshes.is_empty());
        fbx.primitive_meshes[0].clone()
    };

    let meshes = app.world().resource::<Assets<Mesh>>();
    let mesh = meshes.get(&mesh_handle).expect("Mesh0");

    let Some(VertexAttributeValues::Uint16x4(indices)) =
        mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
    else {
        panic!("skinned mesh missing JOINT_INDEX");
    };
    let Some(weights) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) else {
        panic!("skinned mesh missing JOINT_WEIGHT");
    };

    assert_eq!(indices.len(), mesh.count_vertices());
    match weights {
        VertexAttributeValues::Float32x4(w) => {
            assert_eq!(w.len(), indices.len());
            for row in w {
                let sum: f32 = row.iter().sum();
                assert!(
                    (sum - 1.0).abs() < 1e-3 || sum == 0.0,
                    "weights should renormalize to ~1, got {sum}"
                );
            }
        }
        _ => panic!("unexpected joint weight format"),
    }
}
