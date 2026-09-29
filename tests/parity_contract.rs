//! Parity contract tests: the wiring `bevy_gltf` guarantees at runtime must hold
//! for the FBX loader too.
//!
//! Covered contracts (fixtures are in-repo `assets/` unless noted):
//! - asset labels resolve and match the `Fbx` handle lists (`Mesh{N}`,
//!   `Mesh{N}/Primitive{P}`, `Node{N}`, `Material{N}`, `Material{N}/Standard`,
//!   `Skin{N}`, `Skin{N}/InverseBindMatrices`, `Animation{N}`, `Scene0`);
//! - morphs: node `MorphWeights` + mesh child `MeshMorphWeights::Reference(node)`;
//! - skinning: `SkinnedMesh` joint/IBM length agreement, live joint entities,
//!   joint-index attribute values inside range, `MAX_JOINTS` cap;
//! - animation: `AnimationPlayer` on the armature root, `(AnimationTargetId,
//!   AnimatedBy)` on descendants, every clip curve target present in the scene,
//!   and real playback through an `AnimationGraph` moves target transforms;
//! - negative world scale selects the cull-inverted material twin;
//! - skinned-bounds policy metadata (`DynamicSkinnedMeshBounds` / `NoFrustumCulling`);
//! - `FbxSpaceConversion::TransformRoot` conversion survives into the built scene
//!   (synthetic-root wrapper) and matches the default world-space result;
//! - FBX spot cone full-aperture degrees convert to Bevy half-angle radians.

use std::collections::HashMap;
use std::time::Duration;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
use bevy::animation::{
    AnimatedBy, AnimationClip, AnimationPlayer, AnimationPlugin, AnimationTargetId,
};
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::camera::visibility::{DynamicSkinnedMeshBounds, NoFrustumCulling};
use bevy::image::Image;
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::morph::{MeshMorphWeights, MorphWeights};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use bevy::render::render_resource::Face;
use bevy::time::TimeUpdateStrategy;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldSerializationPlugin};
use bevy_ufbx::{
    Fbx, FbxAssetLabel, FbxLoaderSettings, FbxMaterial, FbxMesh, FbxNode, FbxPlugin, FbxSkin,
    FbxSkinnedMeshBoundsPolicy, FbxSpaceConversion,
};

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

/// App able to *spawn* a `WorldAsset` and play animation clips.
///
/// The world-asset spawner requires every component type in the asset world to be
/// registered with `ReflectComponent`, so the component types used by loader
/// scenes are registered explicitly instead of pulling in the render plugins.
fn spawnable_app() -> App {
    let mut app = headless_app();
    app.add_plugins(AnimationPlugin)
        .add_plugins(WorldSerializationPlugin)
        .register_type::<Transform>()
        .register_type::<GlobalTransform>()
        .register_type::<Visibility>()
        .register_type::<Name>()
        .register_type::<Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>()
        .register_type::<AnimationPlayer>()
        .register_type::<AnimationTargetId>()
        .register_type::<AnimatedBy>();
    app
}

fn wait_for_asset<A: Asset>(app: &mut App, handle: &Handle<A>, max_frames: usize) -> LoadState {
    for _ in 0..max_frames {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.load_state(handle) {
            LoadState::Loaded => return LoadState::Loaded,
            state @ LoadState::Failed(_) => return state,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    app.world().resource::<AssetServer>().load_state(handle)
}

#[track_caller]
fn load_fbx(app: &mut App, path: &str) -> Handle<Fbx> {
    let handle: Handle<Fbx> = app.world().resource::<AssetServer>().load(path.to_string());
    match wait_for_asset(app, &handle, 800) {
        LoadState::Loaded => handle,
        state => panic!("{path} failed to load: {state:?}"),
    }
}

fn default_scene(app: &App, fbx_handle: &Handle<Fbx>) -> Handle<WorldAsset> {
    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(fbx_handle).expect("Fbx asset");
    fbx.default_scene.clone().expect("default_scene")
}

fn scene_world<'a>(app: &'a App, scene_handle: &Handle<WorldAsset>) -> &'a World {
    let worlds = app.world().resource::<Assets<WorldAsset>>();
    &worlds.get(scene_handle).expect("Scene0 WorldAsset").world
}

/// Entities of the spawned scene carrying `AnimationTargetId`.
fn scene_animation_targets(
    app: &App,
    scene: &Handle<WorldAsset>,
) -> HashMap<AnimationTargetId, Entity> {
    let world = scene_world(app, scene);
    let mut targets = HashMap::new();
    for entity in world.iter_entities() {
        if let Some(target_id) = world.get::<AnimationTargetId>(entity.id()) {
            targets.insert(*target_id, entity.id());
        }
    }
    targets
}

// ── labels ──────────────────────────────────────────────────────────────────

#[test]
fn asset_labels_resolve_and_match_fbx_handles() {
    let mut app = headless_app();
    let path = "blender_suzanne_multimaterial_7400_binary.fbx";
    let fbx_handle = load_fbx(&mut app, path);

    let server = app.world().resource::<AssetServer>();

    let mesh_label: Handle<FbxMesh> = server.load(FbxAssetLabel::Mesh(0).from_asset(path));
    let prim_label: Handle<Mesh> = server.load(
        FbxAssetLabel::Primitive {
            mesh: 0,
            primitive: 0,
        }
        .from_asset(path),
    );
    let material_label: Handle<FbxMaterial> =
        server.load(FbxAssetLabel::Material(0).from_asset(path));
    let material_std_label: Handle<StandardMaterial> =
        server.load(FbxAssetLabel::MaterialStandard(0).from_asset(path));
    let node_label: Handle<FbxNode> = server.load(FbxAssetLabel::Node(0).from_asset(path));
    let scene_label: Handle<WorldAsset> = server.load(FbxAssetLabel::Scene(0).from_asset(path));

    for (name, state) in [
        ("Mesh0", wait_for_asset(&mut app, &mesh_label, 400)),
        (
            "Mesh0/Primitive0",
            wait_for_asset(&mut app, &prim_label, 400),
        ),
        ("Material0", wait_for_asset(&mut app, &material_label, 400)),
        (
            "Material0/Standard",
            wait_for_asset(&mut app, &material_std_label, 400),
        ),
        ("Node0", wait_for_asset(&mut app, &node_label, 400)),
        ("Scene0", wait_for_asset(&mut app, &scene_label, 400)),
    ] {
        assert!(state.is_loaded(), "{name} label did not resolve: {state:?}");
    }

    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");

    // Label handles must be the very same subassets the `Fbx` root asset exposes.
    assert_eq!(
        fbx.meshes.first().expect("Mesh0 in Fbx.meshes"),
        &mesh_label,
        "Mesh0 label must match Fbx.meshes[0]"
    );
    assert_eq!(
        fbx.materials.first().expect("Material0 in Fbx.materials"),
        &material_label,
        "Material0 label must match Fbx.materials[0]"
    );
    assert_eq!(
        fbx.default_scene.as_ref().expect("default_scene"),
        &scene_label,
        "Scene0 label must match Fbx.default_scene"
    );

    let meshes = app.world().resource::<Assets<FbxMesh>>();
    let mesh = meshes.get(&mesh_label).expect("FbxMesh asset");
    assert!(
        !mesh.primitives.is_empty(),
        "Mesh0 must expose at least one primitive"
    );
    assert!(
        mesh.primitives.iter().any(|p| p.mesh == prim_label),
        "Mesh0/Primitive0 label must match a primitive of the FbxMesh"
    );

    let materials = app.world().resource::<Assets<FbxMaterial>>();
    let material = materials.get(&material_label).expect("FbxMaterial asset");
    assert_eq!(
        material.material, material_std_label,
        "Material0/Standard must be FbxMaterial::material"
    );

    let nodes = app.world().resource::<Assets<FbxNode>>();
    let node = nodes.get(&node_label).expect("FbxNode asset");
    assert!(!node.name.is_empty(), "Node0 should carry a display name");
}

#[test]
fn skin_and_animation_labels_resolve() {
    let mut app = headless_app();

    // skin labels
    let skin_path = "rigged_triangle.fbx";
    let skin_fbx = load_fbx(&mut app, skin_path);
    let skin_label: Handle<FbxSkin> = app
        .world()
        .resource::<AssetServer>()
        .load(FbxAssetLabel::Skin(0).from_asset(skin_path));
    let ibm_label: Handle<SkinnedMeshInverseBindposes> = app
        .world()
        .resource::<AssetServer>()
        .load(FbxAssetLabel::InverseBindMatrices(0).from_asset(skin_path));
    assert!(wait_for_asset(&mut app, &skin_label, 400).is_loaded());
    assert!(wait_for_asset(&mut app, &ibm_label, 400).is_loaded());

    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&skin_fbx).expect("Fbx asset");
    assert_eq!(
        fbx.skins.first().expect("Skin0 in Fbx.skins"),
        &skin_label,
        "Skin0 label must match Fbx.skins[0]"
    );
    let skins = app.world().resource::<Assets<FbxSkin>>();
    let skin = skins.get(&skin_label).expect("FbxSkin asset");
    assert_eq!(
        skin.inverse_bind_matrices, ibm_label,
        "Skin0/InverseBindMatrices must be FbxSkin::inverse_bind_matrices"
    );

    // animation label
    let anim_path = "cube_anim.fbx";
    let anim_fbx = load_fbx(&mut app, anim_path);
    let clip_label: Handle<AnimationClip> = app
        .world()
        .resource::<AssetServer>()
        .load(FbxAssetLabel::Animation(0).from_asset(anim_path));
    assert!(wait_for_asset(&mut app, &clip_label, 400).is_loaded());

    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(&anim_fbx).expect("Fbx asset");
    assert_eq!(
        fbx.animations
            .first()
            .expect("Animation0 in Fbx.animations"),
        &clip_label,
        "Animation0 label must match Fbx.animations[0]"
    );
    assert_eq!(
        fbx.named_animations.get("Take 001"),
        Some(&clip_label),
        "named_animations['Take 001'] must point at Animation0"
    );
    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips.get(&clip_label).expect("AnimationClip asset");
    assert!(
        clip.duration() > 0.0,
        "baked clip must have a positive duration"
    );
    assert!(!clip.curves().is_empty(), "baked clip must have curves");
}

// ── morphs ──────────────────────────────────────────────────────────────────

#[test]
fn morph_weights_reference_the_mesh_entity() {
    let mut app = headless_app();
    let path = "blend_shape_cube.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let world = scene_world(&app, &scene);

    let node_with_weights = world
        .iter_entities()
        .find_map(|e| world.get::<MorphWeights>(e.id()).map(|w| (e.id(), w)))
        .expect("blend_shape_cube.fbx must spawn a MorphWeights node");
    let (node_entity, weights) = node_with_weights;
    assert!(
        !weights.weights().is_empty(),
        "MorphWeights must carry at least one weight"
    );
    assert!(
        weights.first_mesh().is_some(),
        "MorphWeights must reference the first mesh primitive"
    );

    let mesh_handle = weights.first_mesh().expect("first mesh").clone();
    let mesh_asset = app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&mesh_handle)
        .expect("morph mesh asset");

    // Bevy 0.19 packs morph target deltas target-major: `morph_targets()` holds
    // `target_count * vertex_count` `MorphAttributes`, and the target names list is
    // one entry per target. `MorphWeights` must have exactly one weight per target.
    let vertex_count = match mesh_asset.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => positions.len(),
        other => panic!("expected Float32x3 positions on a morph mesh, got {other:?}"),
    };
    let attribute_count = mesh_asset
        .morph_targets()
        .map(Vec::len)
        .expect("morph mesh must have morph targets");
    assert!(
        vertex_count > 0 && attribute_count % vertex_count == 0,
        "morph target packing must be target-major ({attribute_count} attributes / {vertex_count} vertices)"
    );
    let target_count = attribute_count / vertex_count;
    let name_count = mesh_asset
        .morph_target_names()
        .expect("morph target names must be set alongside morph targets")
        .len();
    assert_eq!(
        name_count, target_count,
        "morph target names must have one entry per target"
    );
    assert_eq!(
        weights.weights().len(),
        target_count,
        "MorphWeights must have one weight per morph target ({} weights for {} targets)",
        weights.weights().len(),
        target_count
    );

    let referencing = world
        .iter_entities()
        .filter(|e| {
            matches!(
                world.get::<MeshMorphWeights>(e.id()),
                Some(&MeshMorphWeights::Reference(target)) if target == node_entity
            )
        })
        .count();
    assert!(
        referencing >= 1,
        "at least one mesh entity must carry MeshMorphWeights::Reference(node with MorphWeights)"
    );

    // The referencing entity must be a mesh child of the weights node.
    for entity in world.iter_entities() {
        if let Some(&MeshMorphWeights::Reference(target)) =
            world.get::<MeshMorphWeights>(entity.id())
        {
            assert!(
                world.get::<Mesh3d>(entity.id()).is_some(),
                "MeshMorphWeights::Reference must live on a mesh entity"
            );
            assert_eq!(
                target, node_entity,
                "all morph references in this fixture point at the weights node"
            );
            assert!(
                world.get::<MorphWeights>(target).is_some(),
                "MeshMorphWeights::Reference target must own MorphWeights"
            );
        }
    }
}

// ── skinning ────────────────────────────────────────────────────────────────

#[test]
fn skinned_mesh_joints_ibm_and_index_bounds_are_consistent() {
    let mut app = headless_app();
    let path = "blender_279_sausage_7400_binary.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let world = scene_world(&app, &scene);
    let skinned: Vec<(Entity, SkinnedMesh)> = world
        .iter_entities()
        .filter_map(|e| {
            world
                .get::<SkinnedMesh>(e.id())
                .map(|s| (e.id(), s.clone()))
        })
        .collect();
    assert!(
        !skinned.is_empty(),
        "sausage fixture must spawn at least one SkinnedMesh"
    );

    let ibm_assets = app
        .world()
        .resource::<Assets<SkinnedMeshInverseBindposes>>();
    let mesh_assets = app.world().resource::<Assets<Mesh>>();

    for (entity, skin) in skinned {
        assert!(
            !skin.joints.is_empty(),
            "SkinnedMesh {entity:?} must list joints"
        );
        assert!(
            skin.joints.len() <= 256,
            "joint count {} exceeds Bevy's MAX_JOINTS (256)",
            skin.joints.len()
        );
        let ibms = ibm_assets
            .get(&skin.inverse_bindposes)
            .expect("SkinnedMeshInverseBindposes asset");
        assert_eq!(
            ibms.len(),
            skin.joints.len(),
            "IBM asset length must equal joint count"
        );
        for (i, ibm) in ibms.iter().enumerate() {
            assert!(
                ibm.to_cols_array().iter().all(|v| v.is_finite()),
                "IBM {i} must be finite"
            );
        }

        for joint in &skin.joints {
            assert!(
                world.get_entity(*joint).is_ok(),
                "joint entity {joint:?} must exist in the spawned scene"
            );
        }

        let mesh_handle = world
            .get::<Mesh3d>(entity)
            .expect("skinned entity must have Mesh3d")
            .0
            .clone();
        let mesh = mesh_assets.get(&mesh_handle).expect("skinned mesh asset");
        match mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX) {
            Some(VertexAttributeValues::Uint16x4(indices)) => {
                assert!(
                    !indices.is_empty(),
                    "joint index attribute must not be empty"
                );
                let max_index = indices.iter().flatten().copied().max().unwrap_or(0);
                assert!(
                    (max_index as usize) < skin.joints.len(),
                    "joint index {max_index} out of range for {} joints",
                    skin.joints.len()
                );
            }
            other => panic!("expected Uint16x4 joint indices, got {other:?}"),
        }
    }
}

// ── animation wiring + real playback ────────────────────────────────────────

#[test]
fn animation_player_and_targets_cover_every_clip_curve() {
    let mut app = headless_app();
    let path = "cube_anim.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let clip_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.named_animations
            .get("Take 001")
            .expect("Take 001 clip")
            .clone()
    };
    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips.get(&clip_handle).expect("AnimationClip asset");

    let targets = scene_animation_targets(&app, &scene);
    assert!(
        !targets.is_empty(),
        "animated scene must tag nodes with AnimationTargetId"
    );

    let world = scene_world(&app, &scene);
    let players: Vec<Entity> = world
        .iter_entities()
        .filter(|e| world.get::<AnimationPlayer>(e.id()).is_some())
        .map(|e| e.id())
        .collect();
    assert_eq!(
        players.len(),
        1,
        "cube_anim.fbx has a single armature root, expected exactly one AnimationPlayer"
    );
    let player = players[0];

    // Every curve target in the clip must exist in the scene...
    for target_id in clip.curves().keys() {
        assert!(
            targets.contains_key(target_id),
            "clip curve target {target_id:?} has no matching AnimationTargetId entity in the scene"
        );
    }
    // ...and every tagged target must point at the player, including the root.
    for (target_id, entity) in &targets {
        let animated_by = world
            .get::<AnimatedBy>(*entity)
            .unwrap_or_else(|| panic!("target {target_id:?} is missing AnimatedBy"));
        assert_eq!(
            animated_by.0, player,
            "target {target_id:?} must be AnimatedBy the armature-root player"
        );
    }
    let root_entity = player;
    assert!(
        world.get::<AnimationTargetId>(root_entity).is_some(),
        "the animation root itself must be an animation target (glTF parity)"
    );
}

#[test]
fn animation_playback_moves_targets_after_spawn_and_graph() {
    let mut app = spawnable_app();
    app.world_mut()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));

    let path = "cube_anim.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    // Spawn Scene0 into the app world; the spawner runs in the `SpawnScene` schedule.
    let scene_root = app.world_mut().spawn(WorldAssetRoot(scene)).id();
    for _ in 0..3 {
        app.update();
    }
    assert!(
        app.world().get::<WorldAssetRoot>(scene_root).is_some(),
        "scene root must survive spawning"
    );

    let (target_count, elapsed, moved) = spawn_and_play(&mut app, &fbx_handle);
    assert!(target_count > 0, "spawned scene lost its animation targets");
    assert!(
        elapsed > 0.0,
        "animation must have advanced (player elapsed is 0)"
    );
    assert!(
        moved,
        "playback through the AnimationGraph must move at least one target Transform"
    );
}

/// Attach an `AnimationGraph` for the Fbx's first clip to the spawned player,
/// advance 8 manual-time frames and report `(target count, max elapsed, moved)`.
/// Requires the scene to be spawned into the app world already.
fn spawn_and_play(app: &mut App, fbx_handle: &Handle<Fbx>) -> (usize, f32, bool) {
    let targets = {
        let world = app.world();
        let mut targets = HashMap::new();
        for entity in world.iter_entities() {
            if let Some(target_id) = world.get::<AnimationTargetId>(entity.id()) {
                targets.insert(*target_id, entity.id());
            }
        }
        targets
    };

    let player = app
        .world()
        .iter_entities()
        .find(|e| e.get::<AnimationPlayer>().is_some())
        .map(|e| e.id())
        .expect("spawned scene must contain the AnimationPlayer");

    let (graph, node_index) = AnimationGraph::from_clip(clip_handle(app, fbx_handle));
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    app.world_mut()
        .entity_mut(player)
        .insert(AnimationGraphHandle(graph_handle))
        .get_mut::<AnimationPlayer>()
        .expect("AnimationPlayer")
        .start(node_index);

    let before: Vec<(Entity, Transform)> = targets
        .values()
        .map(|e| {
            (
                *e,
                *app.world().get::<Transform>(*e).expect("target Transform"),
            )
        })
        .collect();

    for _ in 0..8 {
        app.update();
    }

    let elapsed = app
        .world()
        .get::<AnimationPlayer>(player)
        .expect("AnimationPlayer")
        .playing_animations()
        .map(|(_, active)| active.elapsed())
        .fold(0.0f32, f32::max);

    let moved = before.iter().any(|(entity, original)| {
        app.world().get::<Transform>(*entity).is_none_or(|now| {
            now.translation.distance(original.translation) > 1e-4
                || now.rotation.dot(original.rotation).abs() < 1.0 - 1e-4
                || now.scale.distance(original.scale) > 1e-4
        })
    });

    (targets.len(), elapsed, moved)
}

fn clip_handle(app: &App, fbx_handle: &Handle<Fbx>) -> Handle<AnimationClip> {
    let fbxs = app.world().resource::<Assets<Fbx>>();
    let fbx = fbxs.get(fbx_handle).expect("Fbx asset");
    fbx.animations.first().expect("Animation0").clone()
}

// ── synthetic-root space conversion (`FbxSpaceConversion::TransformRoot`) ───

/// Load `path` with an explicit space conversion; returns `(Fbx, Scene0)`.
fn load_scene_with_conversion(
    app: &mut App,
    path: &'static str,
    conversion: FbxSpaceConversion,
) -> (Handle<Fbx>, Handle<WorldAsset>) {
    let handle: Handle<Fbx> = app
        .world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(move |s: &mut FbxLoaderSettings| {
            s.space_conversion = conversion;
        })
        .load(path);
    assert!(
        wait_for_asset(app, &handle, 800).is_loaded(),
        "{path} failed to load with {conversion:?}"
    );
    let scene = default_scene(app, &handle);
    (handle, scene)
}

/// World-space matrix per entity, composed from the stored `Children` hierarchy.
/// Independent of transform propagation, so it also works on a `WorldAsset` world.
fn scene_globals(world: &World) -> HashMap<Entity, Mat4> {
    fn visit(world: &World, entity: Entity, parent: Mat4, out: &mut HashMap<Entity, Mat4>) {
        let local = world
            .get::<Transform>(entity)
            .map(|t| t.to_matrix())
            .unwrap_or(Mat4::IDENTITY);
        let global = parent * local;
        out.insert(entity, global);
        if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter().collect::<Vec<Entity>>() {
                visit(world, child, global, out);
            }
        }
    }

    let mut out = HashMap::new();
    for entity in world.iter_entities() {
        let id = entity.id();
        if world.get::<Transform>(id).is_none() {
            continue;
        }
        if world.get::<ChildOf>(id).is_none() {
            visit(world, id, Mat4::IDENTITY, &mut out);
        }
    }
    out
}

/// World-space AABB of every spawned mesh vertex (`global matrix * position`).
fn world_space_bounds(app: &App, scene: &Handle<WorldAsset>) -> (Vec3, Vec3) {
    let world = scene_world(app, scene);
    let globals = scene_globals(world);
    let meshes = app.world().resource::<Assets<Mesh>>();

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;
    for entity in world.iter_entities() {
        let Some(Mesh3d(mesh_handle)) = world.get::<Mesh3d>(entity.id()) else {
            continue;
        };
        let Some(global) = globals.get(&entity.id()) else {
            continue;
        };
        let Some(mesh) = meshes.get(mesh_handle.id()) else {
            continue;
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        for position in positions {
            let world_position = global.transform_point3(Vec3::from(*position));
            min = min.min(world_position);
            max = max.max(world_position);
            found = true;
        }
    }
    assert!(found, "scene must expose at least one mesh with positions");
    (min, max)
}

#[test]
fn transform_root_conversion_is_carried_by_a_wrapper_and_matches_default() {
    let path = "maya_cube_7400_binary.fbx";

    // Default strategy: conversion baked into geometry (Maya probe -> ModifyGeometry).
    let mut default_app = headless_app();
    let (_default_fbx, default_scene) =
        load_scene_with_conversion(&mut default_app, path, FbxSpaceConversion::ModifyGeometry);
    let (default_min, default_max) = world_space_bounds(&default_app, &default_scene);
    let default_world = scene_world(&default_app, &default_scene);
    assert!(
        default_world.iter_entities().all(|e| default_world
            .get::<Name>(e.id())
            .is_none_or(|n| n.as_str() != "FbxSceneRoot")),
        "ModifyGeometry must not need a synthetic-root wrapper (default hierarchy unchanged)"
    );

    // TransformRoot strategy: the whole conversion lives on the synthetic root.
    let mut root_app = headless_app();
    let (_root_fbx, root_scene) =
        load_scene_with_conversion(&mut root_app, path, FbxSpaceConversion::TransformRoot);
    let (root_min, root_max) = world_space_bounds(&root_app, &root_scene);
    let root_world = scene_world(&root_app, &root_scene);

    let wrapper = root_world
        .iter_entities()
        .find(|e| {
            root_world
                .get::<Name>(e.id())
                .is_some_and(|n| n.as_str() == "FbxSceneRoot")
        })
        .expect("TransformRoot conversion must be carried by the synthetic-root wrapper")
        .id();
    let wrapper_transform = *root_world
        .get::<Transform>(wrapper)
        .expect("wrapper Transform");
    assert!(
        wrapper_transform != Transform::IDENTITY,
        "wrapper must carry the actual conversion, not an identity placeholder"
    );
    let wrapper_children = root_world
        .get::<Children>(wrapper)
        .expect("top-level nodes must be parented to the wrapper");
    assert!(
        !wrapper_children.is_empty(),
        "wrapper must own the top-level scene nodes"
    );
    assert!(
        wrapper_children
            .iter()
            .all(|child| root_world.get::<Transform>(child).is_some()),
        "every wrapper child must be a real scene node"
    );

    // Both strategies must reach the same world space (ufbx guarantee); only the
    // carrier differs, so a silently dropped conversion is visible right here.
    let default_size = default_max - default_min;
    let root_size = root_max - root_min;
    let size_error = ((root_size - default_size).abs() / default_size.abs().max(Vec3::splat(1e-6)))
        .max_element();
    assert!(
        size_error < 1e-2,
        "TransformRoot world-space size {root_size:?} must match ModifyGeometry {default_size:?}"
    );
    assert!(
        (root_min - default_min).length() < 1e-3 * default_size.length().max(1.0),
        "TransformRoot world-space origin {root_min:?} must match ModifyGeometry {default_min:?}"
    );
}

#[test]
fn transform_root_wrapper_is_not_an_animation_target_and_playback_still_works() {
    let mut app = spawnable_app();
    app.world_mut()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));

    let path = "cube_anim.fbx";
    let (fbx_handle, scene) =
        load_scene_with_conversion(&mut app, path, FbxSpaceConversion::TransformRoot);

    {
        let world = scene_world(&app, &scene);
        assert!(
            world.iter_entities().any(|e| world
                .get::<Name>(e.id())
                .is_some_and(|n| n.as_str() == "FbxSceneRoot")),
            "Blender cube_anim.fbx must carry its TransformRoot conversion on the wrapper"
        );
    }

    app.world_mut().spawn(WorldAssetRoot(scene));
    for _ in 0..3 {
        app.update();
    }

    // The wrapper is not part of any animation path (names.rs policy A), so baked
    // curves can never overwrite the conversion by writing the root's TRS.
    {
        let world = app.world();
        for entity in world.iter_entities() {
            let is_wrapper = world
                .get::<Name>(entity.id())
                .is_some_and(|n| n.as_str() == "FbxSceneRoot");
            if is_wrapper {
                assert!(
                    world.get::<AnimationTargetId>(entity.id()).is_none(),
                    "synthetic-root wrapper must not be an animation target"
                );
                assert!(
                    world.get::<AnimationPlayer>(entity.id()).is_none(),
                    "synthetic-root wrapper must not own the AnimationPlayer"
                );
            }
        }
    }

    let (target_count, elapsed, moved) = spawn_and_play(&mut app, &fbx_handle);
    assert!(
        target_count > 0,
        "TransformRoot scene lost its animation targets"
    );
    assert!(elapsed > 0.0, "animation must advance under TransformRoot");
    assert!(
        moved,
        "animation curves must still move targets under TransformRoot"
    );
}

// ── negative scale + bounds policy ──────────────────────────────────────────

fn cull_modes_used(app: &mut App, path: &str) -> Vec<Option<Face>> {
    let fbx_handle = load_fbx(app, path);
    let scene = default_scene(app, &fbx_handle);
    let handles: Vec<Handle<StandardMaterial>> = {
        let world = scene_world(app, &scene);
        world
            .iter_entities()
            .filter_map(|e| {
                world
                    .get::<MeshMaterial3d<StandardMaterial>>(e.id())
                    .map(|m| m.0.clone())
            })
            .collect()
    };
    assert!(!handles.is_empty(), "{path} must reference materials");
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    handles
        .iter()
        .map(|h| materials.get(h).expect("StandardMaterial").cull_mode)
        .collect()
}

#[test]
fn negative_world_scale_selects_inverted_cull_material() {
    let mut app = headless_app();
    let modes = cull_modes_used(&mut app, "blender_340_mirrored_normals_7400_binary.fbx");
    assert!(
        modes.contains(&Some(Face::Front)),
        "mirrored/negative-scale fixture must use the cull-inverted material twin, got {modes:?}"
    );

    let mut app = headless_app();
    let modes = cull_modes_used(&mut app, "maya_cube_7400_binary.fbx");
    assert!(
        !modes.contains(&Some(Face::Front)),
        "non-mirrored fixture must not use the inverted cull twin, got {modes:?}"
    );
}

#[test]
fn skinned_mesh_bounds_policy_controls_metadata_components() {
    let path = "blender_279_sausage_7400_binary.fbx";

    let load_with_policy = |policy: FbxSkinnedMeshBoundsPolicy| -> (bool, bool) {
        let mut app = headless_app();
        let handle: Handle<Fbx> = app
            .world()
            .resource::<AssetServer>()
            .load_builder()
            .with_settings(move |s: &mut FbxLoaderSettings| {
                s.skinned_mesh_bounds_policy = policy;
            })
            .load(path);
        assert!(
            wait_for_asset(&mut app, &handle, 800).is_loaded(),
            "{path} failed to load under policy {policy:?}"
        );
        let scene = default_scene(&app, &handle);
        let world = scene_world(&app, &scene);
        let mut has_dynamic = false;
        let mut has_no_frustum = false;
        for entity in world.iter_entities() {
            if world.get::<SkinnedMesh>(entity.id()).is_some() {
                has_dynamic |= world.get::<DynamicSkinnedMeshBounds>(entity.id()).is_some();
                has_no_frustum |= world.get::<NoFrustumCulling>(entity.id()).is_some();
            }
        }
        (has_dynamic, has_no_frustum)
    };

    assert_eq!(
        load_with_policy(FbxSkinnedMeshBoundsPolicy::Dynamic),
        (true, false),
        "Dynamic policy must add DynamicSkinnedMeshBounds only"
    );
    assert_eq!(
        load_with_policy(FbxSkinnedMeshBoundsPolicy::NoFrustumCulling),
        (false, true),
        "NoFrustumCulling policy must add NoFrustumCulling only"
    );
    assert_eq!(
        load_with_policy(FbxSkinnedMeshBoundsPolicy::BindPose),
        (false, false),
        "BindPose policy must not add bounds metadata components"
    );
}

// ── FBX units: spot cone full aperture degrees → Bevy half-angle radians ────

#[test]
fn spot_cone_angles_are_converted_from_fbx_full_aperture_degrees() {
    let mut app = headless_app();
    let path = "motionbuilder_lights_7700_ascii.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let world = scene_world(&app, &scene);
    let spots: Vec<SpotLight> = world
        .iter_entities()
        .filter_map(|e| world.get::<SpotLight>(e.id()).cloned())
        .collect();
    let points = world
        .iter_entities()
        .filter(|e| world.get::<PointLight>(e.id()).is_some())
        .count();

    assert_eq!(
        spots.len(),
        5,
        "fixture contains 5 MotionBuilder spot lights"
    );
    assert_eq!(points, 4, "fixture contains 4 point lights");

    // The fixture omits ConeAngle/OuterAngle, so ufbx reports 0.0 and the FBX SDK
    // default of 45 degrees full aperture applies: 45° / 2 = 22.5° = PI/8 rad.
    let expected_outer = std::f32::consts::FRAC_PI_8;
    for (i, spot) in spots.iter().enumerate() {
        assert!(
            (spot.outer_angle - expected_outer).abs() < 1e-4,
            "spot {i}: outer_angle {} must be the FBX default 45 deg full aperture as a half angle \
             ({expected_outer} rad), not raw degrees or zero",
            spot.outer_angle
        );
        assert_eq!(spot.inner_angle, 0.0, "spot {i}: inner cone defaults to 0");
        assert!(
            spot.outer_angle < std::f32::consts::FRAC_PI_2,
            "spot {i}: Bevy requires outer_angle < PI/2"
        );
    }
}

#[test]
fn first_camera_in_scene_order_is_active_and_only_it() {
    let mut app = headless_app();
    let path = "maya_camera_light_axes_y_up_6100_binary.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let world = scene_world(&app, &scene);
    let active: Vec<bool> = world
        .iter_entities()
        .filter_map(|e| world.get::<Camera>(e.id()).map(|c| c.is_active))
        .collect();

    assert!(
        !active.is_empty(),
        "fixture carries at least one camera node"
    );
    assert_eq!(
        active.iter().filter(|is_active| **is_active).count(),
        1,
        "bevy_gltf activates exactly one camera per spawned scene (the first node in \
         scene order, `is_active: !*active_camera_found`); the FBX loader must match"
    );
}

#[test]
fn punctual_lights_carry_fbx_range_and_spot_radius_mirrors_it() {
    let mut app = headless_app();
    let path = "motionbuilder_lights_7700_ascii.fbx";
    let fbx_handle = load_fbx(&mut app, path);
    let scene = default_scene(&app, &fbx_handle);

    let world = scene_world(&app, &scene);
    let mut points = Vec::new();
    let mut spots = Vec::new();
    for entity in world.iter_entities() {
        if let Some(point) = world.get::<PointLight>(entity.id()) {
            points.push(point.clone());
        }
        if let Some(spot) = world.get::<SpotLight>(entity.id()) {
            spots.push(spot.clone());
        }
    }
    assert_eq!(points.len(), 4, "fixture contains 4 point lights");
    assert_eq!(spots.len(), 5, "fixture contains 5 spot lights");

    // The fixture authors no attenuation end, so the punctual-light default
    // must apply instead of infinity.
    for (i, point) in points.iter().enumerate() {
        assert_eq!(
            point.range, 20.0,
            "point {i}: unattenuated lights default to the KHR_lights_punctual \
             range used by bevy_gltf"
        );
    }
    for (i, spot) in spots.iter().enumerate() {
        assert_eq!(
            spot.range, 20.0,
            "spot {i}: unattenuated lights default to the KHR_lights_punctual range"
        );
        assert_eq!(
            spot.radius, spot.range,
            "spot {i}: bevy_gltf mirrors the light range onto SpotLight.radius"
        );
    }
}
