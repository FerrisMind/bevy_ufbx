//! Integration tests: skins, IBM labels, joint weights on rigged_triangle.fbx.

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::mesh::{bind_unweighted_vertex, compact_cluster_slots, top4_influences};
use bevy_ufbx::utils::convert_matrix;
use bevy_ufbx::{
    Fbx, FbxAssetLabel, FbxLoaderSettings, FbxPlugin, FbxSkin, FbxSkinnedMeshBoundsPolicy,
};

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

// ============================================================================
// L2: cluster remapping, influence-table weights, canonical IBM, skinned bounds
// ============================================================================

/// `compact_cluster_slots` is the shared remap: dropped clusters leave gaps that
/// must be closed so influence slots line up with the IBM/joints order.
#[test]
fn compact_cluster_slots_remaps_dropped_clusters_to_dense_slots() {
    // Cluster 0 and 3 unusable (missing bone / non-finite IBM).
    let slots = compact_cluster_slots(5, |i| i != 0 && i != 3, 256);
    assert_eq!(
        slots,
        vec![None, Some(0), Some(1), None, Some(2)],
        "valid clusters must compact to dense joint slots in cluster order"
    );

    // No clusters at all.
    assert_eq!(
        compact_cluster_slots(0, |_| true, 256),
        Vec::<Option<u16>>::new()
    );

    // Cap: the first `max_joints` valid clusters survive, the rest are dropped.
    assert_eq!(
        compact_cluster_slots(4, |_| true, 2),
        vec![Some(0), Some(1), None, None]
    );
    // Invalid clusters do not consume cap budget.
    assert_eq!(
        compact_cluster_slots(5, |i| i % 2 == 1, 2),
        vec![None, Some(0), None, Some(1), None]
    );
}

fn weight(cluster_index: u32, weight: f64) -> ufbx::SkinWeight {
    ufbx::SkinWeight {
        cluster_index,
        weight,
    }
}

/// Influence extraction: first four positive finite weights, remapped and
/// renormalized; unknown / dropped clusters and bad weights are discarded. An all-zero row
/// is the extractor's only signal for "no usable influence"; `bind_unweighted_vertex` owns
/// the caller's policy for it.
#[test]
fn top4_influences_filters_renormalizes_and_remaps() {
    let slots = compact_cluster_slots(6, |i| i != 1, 256); // cluster 1 dropped

    // ufbx guarantees descending order. Cluster 1 is dropped, cluster 4 carries
    // NaN, one weight is zero, and the last entry is a 5th positive survivor.
    let weights = [
        weight(0, 0.4),
        weight(1, 0.35), // dropped cluster: must not consume a slot
        weight(2, 0.3),
        weight(4, f64::NAN),
        weight(5, 0.0),
        weight(3, 0.2),
        weight(5, 0.1),
        weight(2, 0.05), // 5th positive survivor: must be cut by the 4 cap
    ];
    let (indices, out) = top4_influences(&weights, &slots);
    assert_eq!(indices, [0, 1, 2, 4]);
    assert_eq!(out, [0.4, 0.3, 0.2, 0.1]);
    assert!((out.iter().sum::<f32>() - 1.0).abs() < 1e-6);

    // Every influence references a dropped / out-of-range cluster: zeroed row.
    let dropped = [weight(1, 0.7), weight(9, 0.3)];
    let (indices, out) = top4_influences(&dropped, &slots);
    assert_eq!(indices, [0, 0, 0, 0]);
    assert_eq!(out, [0.0; 4]);

    // Empty weight slice (vertex with no influences) also yields a zeroed row.
    assert_eq!(top4_influences(&[], &slots), ([0; 4], [0.0; 4]));
}

/// Regression (F1): a vertex whose influences all point at dropped clusters used to keep an
/// all-zero row, and Bevy's `skin_model` — a weighted sum of joint matrices — turns that
/// into the zero matrix, collapsing the vertex to the world origin. It must instead bind
/// rigidly to slot 0, which is the first *valid* cluster because `compact_cluster_slots`
/// assigns dense slots from 0 and matches IBM row 0 in `node::process_skins`.
#[test]
fn unweighted_vertex_binds_to_first_valid_joint_not_the_origin() {
    // Cluster 0 is invalid, so the first valid cluster is cluster 1 => slot 0.
    let slots = compact_cluster_slots(4, |i| i != 0, 256);
    assert_eq!(slots, vec![None, Some(0), Some(1), Some(2)]);
    let has_valid_joint = slots.iter().any(Option::is_some);
    assert!(has_valid_joint);

    // The row the extractor produces when every influence is dropped or out of range.
    let dropped = [weight(0, 0.7), weight(9, 0.3)];
    let (mut indices, mut weights) = top4_influences(&dropped, &slots);
    assert_eq!(weights, [0.0; 4]);
    assert!(bind_unweighted_vertex(
        &mut indices,
        &mut weights,
        has_valid_joint
    ));
    assert_eq!(indices, [0, 0, 0, 0]);
    assert_eq!(
        weights,
        [1.0, 0.0, 0.0, 0.0],
        "an unweighted vertex must follow joint slot 0 rigidly, not the zero matrix"
    );
    assert_eq!(
        weights.iter().sum::<f32>(),
        1.0,
        "the fallback row must remain a valid normalized influence"
    );

    // A vertex with no table entry at all takes the same fallback.
    let (mut indices, mut weights) = top4_influences(&[], &slots);
    assert!(bind_unweighted_vertex(
        &mut indices,
        &mut weights,
        has_valid_joint
    ));
    assert_eq!(weights, [1.0, 0.0, 0.0, 0.0]);

    // A genuine single-influence bind normalizes to the same numbers, so detection keys on
    // the all-zero row and never reports a real rigid bind as a fallback.
    let real = [weight(2, 0.25)];
    let (mut indices, mut weights) = top4_influences(&real, &slots);
    assert_eq!(indices, [1, 0, 0, 0]);
    assert!(
        !bind_unweighted_vertex(&mut indices, &mut weights, has_valid_joint),
        "a real influence must never be reported as a fallback"
    );
    assert_eq!(indices, [1, 0, 0, 0]);
    assert_eq!(weights, [1.0, 0.0, 0.0, 0.0]);
}

/// With no usable cluster at all nothing may be fabricated: `node::process_skins` drops the
/// skin in that case, so the row stays zeroed and the mesh path reports no fallback.
#[test]
fn no_valid_joint_keeps_the_row_zeroed_without_fabricating_a_binding() {
    let slots = compact_cluster_slots(4, |_| false, 256);
    assert!(slots.iter().all(Option::is_none));
    let has_valid_joint = slots.iter().any(Option::is_some);

    let influences = [weight(0, 0.5), weight(1, 0.5)];
    let empty: &[ufbx::SkinWeight] = &[];
    for source in [&influences[..], empty] {
        let (mut indices, mut weights) = top4_influences(source, &slots);
        assert!(!bind_unweighted_vertex(
            &mut indices,
            &mut weights,
            has_valid_joint
        ));
        assert_eq!(indices, [0, 0, 0, 0]);
        assert_eq!(
            weights, [0.0; 4],
            "no joint exists, so no binding may be fabricated"
        );
    }
}

/// Load options mirroring the loader's default path for a Blender binary file
/// (`FbxLoaderSettings::default()` → `FbxSpaceConversion::Auto` → `AdjustTransforms`).
fn blender_default_ufbx_opts() -> ufbx::LoadOpts<'static> {
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
    }
}

fn matrix_max_abs_delta(a: &Mat4, b: &Mat4) -> f32 {
    a.to_cols_array()
        .iter()
        .zip(b.to_cols_array().iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

/// Regression: the IBM must be ufbx's `geometry_to_bone`, not
/// `bind_to_world⁻¹ * geometry_to_world`. On `blender_279_sausage_7400_binary.fbx`
/// the file's load pose differs from its bind pose, so the old formula carries a
/// stray `bind⁻¹ * bone_load` factor (relative error up to ~1.3 per cluster).
#[test]
fn ibm_matches_ufbx_geometry_to_bone_on_posed_fixture() {
    const FIXTURE: &str = "blender_279_sausage_7400_binary.fbx";
    let bytes = std::fs::read(format!("assets/{FIXTURE}")).expect("fixture bytes");

    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(FIXTURE)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "{FIXTURE} failed to load"
    );

    let (ibm_handle, joint_count) = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx");
        let skin_handle = fbx.skins.first().expect("skin").clone();
        let skins = app.world().resource::<Assets<FbxSkin>>();
        let skin = skins.get(&skin_handle).expect("Skin0");
        (
            skin.inverse_bind_matrices.clone(),
            skin.joint_element_ids.len(),
        )
    };

    let root = ufbx::load_memory(&bytes, blender_default_ufbx_opts()).expect("ufbx load");
    let scene: &ufbx::Scene = &root;
    let skin = scene
        .nodes
        .as_ref()
        .iter()
        .find_map(|node| node.mesh.as_ref().map(|m| m.as_ref()))
        .and_then(|mesh| mesh.skin_deformers.first())
        .expect("ufbx skin deformer");

    assert_eq!(
        joint_count,
        skin.clusters.len(),
        "every cluster in this fixture is valid"
    );

    let ibms = app
        .world()
        .resource::<Assets<SkinnedMeshInverseBindposes>>();
    let ibms = ibms.get(&ibm_handle).expect("IBM asset");
    assert_eq!(ibms.len(), skin.clusters.len());

    let mut worst = 0.0f32;
    let mut old_formula_worst = 0.0f32;
    for (index, cluster) in skin.clusters.iter().enumerate() {
        let expected = convert_matrix(&cluster.geometry_to_bone);
        let delta = matrix_max_abs_delta(&ibms[index], &expected);
        worst = worst.max(delta);

        let old_formula = convert_matrix(&cluster.bind_to_world).inverse()
            * convert_matrix(&cluster.geometry_to_world);
        old_formula_worst = old_formula_worst.max(matrix_max_abs_delta(&old_formula, &expected));
    }

    assert!(
        worst < 1e-4,
        "IBM must equal cluster.geometry_to_bone (worst delta {worst})"
    );
    assert!(
        old_formula_worst > 1e-3,
        "fixture must remain sensitive to the old bind_to_world⁻¹ formula (worst delta {old_formula_worst})"
    );
}

/// Dynamic bounds policy (the default) must bake `SkinnedMeshBounds` onto the
/// mesh asset like `bevy_gltf`; `BindPose` / `NoFrustumCulling` must not.
#[test]
fn dynamic_policy_generates_finite_skinned_mesh_bounds() {
    fn load_with_policy(
        app: &mut App,
        policy: FbxSkinnedMeshBoundsPolicy,
    ) -> (Handle<Fbx>, Handle<Mesh>, usize) {
        let fbx_handle: Handle<Fbx> = {
            let server = app.world().resource::<AssetServer>();
            server
                .load_builder()
                .with_settings(move |settings: &mut FbxLoaderSettings| {
                    settings.skinned_mesh_bounds_policy = policy;
                })
                .load("rigged_triangle.fbx")
        };
        assert!(
            wait_for_load(app, &fbx_handle, 500),
            "rigged_triangle.fbx failed to load"
        );
        let (mesh_handle, joints) = {
            let fbxs = app.world().resource::<Assets<Fbx>>();
            let fbx = fbxs.get(&fbx_handle).expect("Fbx");
            let skin = {
                let skins = app.world().resource::<Assets<FbxSkin>>();
                skins
                    .get(fbx.skins.first().expect("skin"))
                    .expect("Skin0")
                    .joints
                    .len()
            };
            (fbx.primitive_meshes[0].clone(), skin)
        };
        (fbx_handle, mesh_handle, joints)
    }

    let mut app = headless_app();
    let (_, mesh_handle, joint_count) =
        load_with_policy(&mut app, FbxSkinnedMeshBoundsPolicy::Dynamic);
    assert!(joint_count > 0);

    let meshes = app.world().resource::<Assets<Mesh>>();
    let mesh = meshes.get(&mesh_handle).expect("Mesh0");
    let bounds = mesh
        .skinned_mesh_bounds()
        .expect("Dynamic policy must bake SkinnedMeshBounds onto the mesh");
    assert!(
        !bounds.aabbs.is_empty(),
        "rigged_triangle has positive weights, so at least one joint AABB"
    );
    assert_eq!(bounds.aabbs.len(), bounds.aabb_index_to_joint_index.len());
    for (joint_index, aabb) in bounds.iter() {
        assert!(
            (joint_index.0 as usize) < joint_count,
            "joint slot {} out of range for {joint_count} joints",
            joint_index.0
        );
        assert!(
            aabb.center.is_finite() && aabb.half_size.is_finite(),
            "generated skinned bounds must be finite: {:?}",
            aabb
        );
    }

    for policy in [
        FbxSkinnedMeshBoundsPolicy::BindPose,
        FbxSkinnedMeshBoundsPolicy::NoFrustumCulling,
    ] {
        let mut app = headless_app();
        let (_, mesh_handle, _) = load_with_policy(&mut app, policy);
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh_handle).expect("Mesh0");
        assert!(
            mesh.skinned_mesh_bounds().is_none(),
            "{policy:?} must not bake skinned mesh bounds"
        );
    }
}
