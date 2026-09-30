//! Headless inspection of a loaded FBX asset.
//!
//! Loads the file through the full Bevy asset pipeline (no window), spawns
//! `Scene0` via `WorldAssetRoot`, and prints everything the loader produced:
//! asset-label lists and label resolution, mesh/primitive details, the scene
//! root with extras and the `FbxSceneRoot` wrapper, per-entity `Aabb`s,
//! cameras, lights, material scalars/textures, extras blobs and — on failure —
//! the granular `FbxError`.
//!
//! ```sh
//! cargo run --example dump_fbx -- my_model.fbx
//! cargo run --example dump_fbx -- "C:\models\My Idle.fbx"
//! ```
//!
//! The path may be `assets/`-relative or absolute (spaces fine — quote it).
//! Absolute paths are *unapproved* for bevy_asset and Bevy 0.19's default
//! `UnapprovedPathMode::Forbid` rejects them before the loader runs, so this
//! "inspect any file" dev tool opts in app-wide with `UnapprovedPathMode::Allow`
//! (the Bevy docs discourage `Allow` for apps with scripts/modding — this
//! example has neither; use `LoadBuilder::override_unapproved()` there).

use std::collections::HashMap;
use std::time::Duration;

use bevy::animation::animation_curves::AnimationCurve;
use bevy::animation::{
    AnimatedBy, AnimationClip, AnimationPlayer, AnimationPlugin, AnimationTargetId,
};
use bevy::asset::io::{AssetSourceBuilder, file::FileAssetReader};
use bevy::asset::{
    AssetApp, AssetLoadError, AssetPlugin, LoadState, RenderAssetUsages, UnapprovedPathMode,
};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{DynamicSkinnedMeshBounds, NoFrustumCulling};
use bevy::image::Image;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldSerializationPlugin};
use bevy_ufbx::{
    Fbx, FbxAssetLabel, FbxError, FbxExtras, FbxLoaderSettings, FbxMaterial, FbxMesh, FbxNode,
    FbxPlugin, FbxSceneExtras, FbxSceneName, FbxSkin, FbxSkinnedMeshBoundsPolicy,
    FbxSpaceConversion,
};

/// Frames to wait for the primary `Fbx` load (10 ms sleep per frame).
const LOAD_TIMEOUT_FRAMES: usize = 6000;
/// Frames to wait for each labeled sub-asset demo load.
const LABEL_TIMEOUT_FRAMES: usize = 800;
/// Frames to wait for the spawned `Scene0` root entity to appear.
const SPAWN_TIMEOUT_FRAMES: usize = 300;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cube.fbx".to_string());

    let mut app = App::new();

    // Section 10's settings matrix: bevy_asset keys a load by the full
    // AssetPath (source + path + label) and returns the existing handle once a
    // path is loaded, so the FIRST settings applied to a path win. Each
    // `v_*://` variant source below is a separate path key over the same
    // `assets/` bytes, giving every (fixture, settings) pair its own loader
    // run. Must be registered BEFORE AssetPlugin builds the AssetServer
    // (same pattern as examples/sampler_settings_fbx.rs).
    for id in MATRIX_VARIANT_SOURCES {
        app.register_asset_source(
            id,
            AssetSourceBuilder::new(|| Box::new(FileAssetReader::new("assets"))),
        );
    }

    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            // "Load any file" dev tool: accept absolute paths. Bevy's default
            // `UnapprovedPathMode::Forbid` rejects them BEFORE the loader runs
            // (see `UnapprovedPathMode` docs for the modding-support caveat that
            // does not apply to this local example).
            unapproved_path_mode: UnapprovedPathMode::Allow,
            ..default()
        },
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>()
    .init_asset::<Image>()
    .init_asset::<AnimationClip>()
    .init_asset::<WorldAsset>()
    .init_asset::<SkinnedMeshInverseBindposes>()
    .add_plugins(FbxPlugin)
    // Headless Scene0 spawn support (mirrors tests/parity_contract.rs
    // `spawnable_app`): reflection registration for every component type
    // the loader scenes contain, so the world spawner can clone them.
    .add_plugins(AnimationPlugin)
    .add_plugins(WorldSerializationPlugin)
    .register_type::<Transform>()
    .register_type::<GlobalTransform>()
    .register_type::<Visibility>()
    .register_type::<Name>()
    .register_type::<ChildOf>()
    .register_type::<Children>()
    .register_type::<Mesh3d>()
    .register_type::<MeshMaterial3d<StandardMaterial>>()
    .register_type::<Aabb>()
    .register_type::<Camera>()
    .register_type::<Camera3d>()
    .register_type::<Projection>()
    .register_type::<DirectionalLight>()
    .register_type::<PointLight>()
    .register_type::<SpotLight>()
    .register_type::<AnimationPlayer>()
    .register_type::<AnimationTargetId>()
    .register_type::<AnimatedBy>();

    let handle: Handle<Fbx> = app.world().resource::<AssetServer>().load(path.clone());
    match wait_for_asset(&mut app, &handle, LOAD_TIMEOUT_FRAMES) {
        LoadState::Loaded => {}
        LoadState::Failed(err) => {
            print_error_section(&path, &err);
            std::process::exit(1);
        }
        state => {
            println!("=== 9. Error display ===");
            println!("load of '{path}' did not finish: {state:?}");
            std::process::exit(1);
        }
    }

    print_label_lists(&mut app, &handle, &path);
    print_mesh_details(&app, &handle);
    print_scene_root(&mut app, &handle, &path);
    print_aabbs(&mut app);
    print_cameras(&mut app);
    print_lights(&mut app);
    print_materials(&app, &handle);
    print_extras(&mut app, &handle);
    println!("=== 9. Error display ===");
    println!(
        "(no errors — load succeeded; on failure this example prints the \
         granular FbxError here and exits 1)"
    );
    print_settings_matrix(&mut app);
}

// ============================================================================
// Load waiting / state helpers
// ============================================================================

fn wait_for_asset<A: Asset>(app: &mut App, handle: &Handle<A>, max_frames: usize) -> LoadState {
    for _ in 0..max_frames {
        app.update();
        let state = app.world().resource::<AssetServer>().load_state(handle);
        match state {
            LoadState::Loaded => return LoadState::Loaded,
            state @ LoadState::Failed(_) => return state,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    app.world().resource::<AssetServer>().load_state(handle)
}

fn state_str(state: &LoadState) -> &'static str {
    match state {
        LoadState::NotLoaded => "NotLoaded",
        LoadState::Loading => "Loading",
        LoadState::Loaded => "Loaded",
        LoadState::Failed(_) => "Failed",
    }
}

fn fbx_ref<'w>(world: &'w World, handle: &Handle<Fbx>) -> &'w Fbx {
    world
        .resource::<Assets<Fbx>>()
        .get(handle)
        .expect("Fbx asset missing after successful load")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// `Some(non-empty blob)` -> the blob, `None`/empty -> `(unauthored)`.
fn extras_display(value: Option<&str>) -> String {
    match value.map(str::trim) {
        None | Some("") => "(unauthored)".to_string(),
        Some(blob) => blob.to_string(),
    }
}

fn name_or(entity_name: Option<&Name>, fallback: &str) -> String {
    entity_name.map_or_else(|| fallback.to_string(), |n| n.as_str().to_string())
}

// ============================================================================
// Section 1: Fbx asset label lists + label resolution demo
// ============================================================================

fn print_label_lists(app: &mut App, handle: &Handle<Fbx>, path: &str) {
    println!("=== 1. Fbx asset label lists ===");
    let (has_meshes, has_materials, has_animations) = {
        let world = app.world();
        let fbx = fbx_ref(world, handle);
        let fbx_meshes = world.resource::<Assets<FbxMesh>>();
        let fbx_materials = world.resource::<Assets<FbxMaterial>>();
        let fbx_nodes = world.resource::<Assets<FbxNode>>();
        let fbx_skins = world.resource::<Assets<FbxSkin>>();
        let clips = world.resource::<Assets<AnimationClip>>();
        let meshes = world.resource::<Assets<Mesh>>();

        println!(
            "  counts: scenes={} meshes={} primitives={} materials={} nodes={} \
             skins={} animations={}",
            fbx.scenes.len(),
            fbx.meshes.len(),
            fbx.primitive_meshes.len(),
            fbx.materials.len(),
            fbx.nodes.len(),
            fbx.skins.len(),
            fbx.animations.len(),
        );
        match fbx
            .default_scene
            .as_ref()
            .and_then(|s| fbx.scenes.iter().position(|h| h.id() == s.id()))
        {
            Some(index) => println!("  default_scene: Scene{index}"),
            None => println!("  default_scene: none"),
        }
        println!(
            "  axis_system: up=({:.3}, {:.3}, {:.3}) front=({:.3}, {:.3}, {:.3}) \
             handedness={:?} unit_scale={:.6}",
            fbx.axis_system.up.x,
            fbx.axis_system.up.y,
            fbx.axis_system.up.z,
            fbx.axis_system.front.x,
            fbx.axis_system.front.y,
            fbx.axis_system.front.z,
            fbx.axis_system.handedness,
            fbx.unit_scale,
        );

        for (i, scene) in fbx.scenes.iter().enumerate() {
            let name = fbx
                .named_scenes
                .iter()
                .find(|(_, h)| h.id() == scene.id())
                .map(|(k, _)| k.to_string());
            println!(
                "  scenes[{i}]: \"{}\"",
                name.unwrap_or_else(|| "(unnamed)".to_string())
            );
        }
        for (i, mesh) in fbx.meshes.iter().enumerate() {
            let name = fbx_meshes.get(mesh).map(|m| m.name.clone());
            println!(
                "  meshes[{i}]: \"{}\"",
                name.unwrap_or_else(|| "(missing FbxMesh)".to_string())
            );
        }
        for (i, prim) in fbx.primitive_meshes.iter().enumerate() {
            let primitive = meshes.get(prim);
            let verts = primitive.map(Mesh::count_vertices).unwrap_or(0);
            let joints = primitive
                .and_then(|m| m.attribute(Mesh::ATTRIBUTE_JOINT_INDEX))
                .map(|a| a.len())
                .unwrap_or(0);
            println!("  primitive_meshes[{i}]: {verts} vertices, {joints} joint indices");
        }
        for (i, material) in fbx.materials.iter().enumerate() {
            let name = fbx_materials.get(material).map(|m| m.name.clone());
            println!(
                "  materials[{i}]: \"{}\"",
                name.unwrap_or_else(|| "(missing FbxMaterial)".to_string())
            );
        }
        for (i, node) in fbx.nodes.iter().enumerate() {
            match fbx_nodes.get(node) {
                Some(n) => println!(
                    "  nodes[{i}]: \"{}\" children={} mesh={} skin={} visible={}",
                    n.name,
                    n.children.len(),
                    n.mesh.is_some(),
                    n.skin.is_some(),
                    n.visible,
                ),
                None => println!("  nodes[{i}]: (missing FbxNode)"),
            }
        }
        for (i, skin) in fbx.skins.iter().enumerate() {
            match fbx_skins.get(skin) {
                Some(s) => println!("  skins[{i}]: \"{}\" joints={}", s.name, s.joints.len()),
                None => println!("  skins[{i}]: (missing FbxSkin)"),
            }
        }
        for (i, clip) in fbx.animations.iter().enumerate() {
            match clips.get(clip) {
                Some(c) => println!(
                    "  animations[{i}]: duration={:.4}s curves={}",
                    c.duration(),
                    c.curves().len()
                ),
                None => println!("  animations[{i}]: (missing AnimationClip)"),
            }
        }

        println!("  named maps (sorted keys -> list entries):");
        print_named("named_scenes", &fbx.named_scenes, &fbx.scenes, "scenes");
        print_named("named_meshes", &fbx.named_meshes, &fbx.meshes, "meshes");
        print_named(
            "named_materials",
            &fbx.named_materials,
            &fbx.materials,
            "materials",
        );
        print_named("named_nodes", &fbx.named_nodes, &fbx.nodes, "nodes");
        print_named("named_skins", &fbx.named_skins, &fbx.skins, "skins");
        print_named(
            "named_animations",
            &fbx.named_animations,
            &fbx.animations,
            "animations",
        );

        (
            !fbx.meshes.is_empty(),
            !fbx.materials.is_empty(),
            !fbx.animations.is_empty(),
        )
    };

    println!("  label resolution (FbxAssetLabel::from_asset + label-based load):");
    demo_label::<WorldAsset>(app, FbxAssetLabel::Scene(0), path);
    if has_meshes {
        demo_label::<FbxMesh>(app, FbxAssetLabel::Mesh(0), path);
    } else {
        absent_label(FbxAssetLabel::Mesh(0), path, "file has no meshes");
    }
    if has_materials {
        demo_label::<FbxMaterial>(app, FbxAssetLabel::Material(0), path);
    } else {
        absent_label(FbxAssetLabel::Material(0), path, "file has no materials");
    }
    if has_animations {
        demo_label::<AnimationClip>(app, FbxAssetLabel::Animation(0), path);
    } else {
        absent_label(FbxAssetLabel::Animation(0), path, "file has no animations");
    }
}

fn print_named<T: Asset>(
    title: &str,
    named: &HashMap<Box<str>, Handle<T>>,
    pool: &[Handle<T>],
    pool_name: &str,
) {
    let mut keys: Vec<&Box<str>> = named.keys().collect();
    keys.sort();
    if keys.is_empty() {
        println!("    {title}: (none)");
        return;
    }
    for key in keys {
        match named.get(key) {
            Some(handle) => {
                let index = pool.iter().position(|h| h.id() == handle.id());
                match index {
                    Some(i) => println!("    {title}[{key}] -> {pool_name}[{i}]"),
                    None => println!("    {title}[{key}] -> (not in {pool_name})"),
                }
            }
            None => println!("    {title}[{key}]: (missing)"),
        }
    }
}

fn demo_label<A: Asset>(app: &mut App, label: FbxAssetLabel, path: &str) {
    let asset_path = label.from_asset(path.to_string());
    let handle: Handle<A> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_path.clone());
    let state = wait_for_asset(app, &handle, LABEL_TIMEOUT_FRAMES);
    println!(
        "    FbxAssetLabel::{label:?}.from_asset({path:?}) -> {asset_path} [{}]",
        state_str(&state)
    );
}

fn absent_label(label: FbxAssetLabel, path: &str, reason: &str) {
    let asset_path = label.from_asset(path.to_string());
    println!(
        "    FbxAssetLabel::{label:?}.from_asset({path:?}) -> {asset_path} [absent — {reason}]"
    );
}

// ============================================================================
// Section 2: FbxMesh details
// ============================================================================

fn print_mesh_details(app: &App, handle: &Handle<Fbx>) {
    println!("=== 2. FbxMesh details ===");
    let world = app.world();
    let fbx = fbx_ref(world, handle);
    let fbx_meshes = world.resource::<Assets<FbxMesh>>();
    let fbx_materials = world.resource::<Assets<FbxMaterial>>();

    if fbx.meshes.is_empty() {
        println!("  (no FbxMesh entries)");
        return;
    }

    // Which Fbx.materials entry owns each non-inverted StandardMaterial handle.
    let material_owner: HashMap<AssetId<StandardMaterial>, usize> = fbx
        .materials
        .iter()
        .enumerate()
        .filter_map(|(i, h)| fbx_materials.get(h).map(|m| (m.material.id(), i)))
        .collect();

    for (i, mesh_handle) in fbx.meshes.iter().enumerate() {
        let Some(mesh) = fbx_meshes.get(mesh_handle) else {
            println!("  meshes[{i}]: (missing FbxMesh asset)");
            continue;
        };
        println!(
            "  meshes[{i}] \"{}\" ({} primitives):",
            mesh.name,
            mesh.primitives.len()
        );
        for (p, prim) in mesh.primitives.iter().enumerate() {
            let material = match prim.material.as_ref() {
                None => "material=None".to_string(),
                Some(std_handle) => {
                    let resolved = std_handle
                        .path()
                        .map(|asset_path| asset_path.to_string())
                        .unwrap_or_else(|| "(uuid handle)".to_string());
                    match material_owner.get(&std_handle.id()) {
                        Some(&mi) => {
                            format!("material=Some -> FbxAssetLabel::Material({mi}) = {resolved}")
                        }
                        None => format!(
                            "material=Some -> unwrapped StandardMaterial (inverted twin / \
                             DefaultMaterial) = {resolved}"
                        ),
                    }
                }
            };
            println!("    primitives[{p}]: name={:?} {material}", prim.name);
        }
    }
}

// ============================================================================
// Section 3: Scene root (spawn Scene0 via WorldAssetRoot)
// ============================================================================

fn print_scene_root(app: &mut App, handle: &Handle<Fbx>, path: &str) {
    println!("=== 3. Scene root (Scene0 via WorldAssetRoot) ===");
    {
        let world = app.world();
        let fbx = fbx_ref(world, handle);
        if fbx.scenes.is_empty() {
            println!("  (file has no scenes)");
            return;
        }
    }

    let scene_label = FbxAssetLabel::Scene(0);
    let asset_path = scene_label.from_asset(path.to_string());
    println!("  spawn via: FbxAssetLabel::{scene_label:?}.from_asset({path:?}) = {asset_path}");
    let scene_handle: Handle<WorldAsset> = app.world().resource::<AssetServer>().load(asset_path);
    let state = wait_for_asset(app, &scene_handle, LABEL_TIMEOUT_FRAMES);
    if !matches!(state, LoadState::Loaded) {
        println!(
            "  Scene(0) load state: {} (spawn skipped)",
            state_str(&state)
        );
        return;
    }

    app.world_mut().spawn(WorldAssetRoot(scene_handle));
    let mut spawned = false;
    for _ in 0..SPAWN_TIMEOUT_FRAMES {
        if scene_root_entity(app).is_some() {
            spawned = true;
            break;
        }
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    if !spawned {
        println!("  scene root entity did not appear after spawn");
        return;
    }
    print_scene_root_components(app);
}

fn scene_root_entity(app: &mut App) -> Option<Entity> {
    let world = app.world_mut();
    let mut roots = world.query_filtered::<(Entity, &FbxSceneName), ()>();
    roots.iter(world).next().map(|(entity, _)| entity)
}

fn print_scene_root_components(app: &mut App) {
    let world = app.world_mut();
    let mut roots = world.query_filtered::<(Entity, &Name, &FbxSceneName, &FbxSceneExtras), ()>();
    let found = roots
        .iter(world)
        .next()
        .map(|(e, n, s, x)| (e, n.as_str().to_string(), s.0.clone(), x.value.clone()));
    let Some((entity, name, scene_name, extras)) = found else {
        println!("  scene root: (not found)");
        return;
    };

    println!("  scene root entity: {entity:?}");
    println!("    Name: {name}");
    println!("    FbxSceneName: {scene_name}");
    println!(
        "    FbxSceneExtras: {}",
        extras_display(Some(extras.as_str()))
    );
    match world.get::<ChildOf>(entity) {
        Some(child_of) => {
            let parent = child_of.parent();
            let under_root = world.get::<WorldAssetRoot>(parent).is_some();
            println!(
                "    ChildOf parent: {parent:?} (WorldAssetRoot marker: {})",
                yes_no(under_root)
            );
        }
        None => println!("    ChildOf parent: (none)"),
    }

    // `FbxSceneRoot` wrapper exists only when the synthetic ufbx root transform
    // converts to a non-identity Transform (unit/axis space conversion).
    let mut wrappers = world.query_filtered::<(Entity, &Transform, &Name), ()>();
    let wrapper = wrappers
        .iter(world)
        .find(|(_, _, n)| n.as_str() == "FbxSceneRoot")
        .map(|(e, _, _)| e);
    match wrapper {
        Some(wrapper_entity) => match world.get::<Transform>(wrapper_entity) {
            Some(t) => println!(
                "    FbxSceneRoot wrapper: present ({wrapper_entity:?}) translation={:?} \
                 rotation={:?} scale={:?}",
                t.translation, t.rotation, t.scale
            ),
            None => println!("    FbxSceneRoot wrapper: present ({wrapper_entity:?})"),
        },
        None => println!(
            "    FbxSceneRoot wrapper: absent (scene root transform is identity — \
             Auto/ModifyGeometry/AdjustTransforms path)"
        ),
    }
}

// ============================================================================
// Section 4: Per-entity Aabb
// ============================================================================

fn print_aabbs(app: &mut App) {
    println!("=== 4. Per-entity Aabb ===");
    let world = app.world_mut();

    let mut aabb_query = world.query_filtered::<(Entity, Option<&Name>, &Aabb), ()>();
    let aabbs: Vec<(Entity, String, [f32; 6])> = aabb_query
        .iter(world)
        .map(|(entity, name, aabb)| {
            let min = aabb.min();
            let max = aabb.max();
            (
                entity,
                name_or(name, "(unnamed)"),
                [min.x, min.y, min.z, max.x, max.y, max.z],
            )
        })
        .collect();

    let mut mesh_query = world.query_filtered::<(Entity, Option<&Name>, &Mesh3d), ()>();
    let mesh_entities: Vec<(Entity, String)> = mesh_query
        .iter(world)
        .map(|(entity, name, _)| (entity, name_or(name, "(unnamed)")))
        .collect();

    if aabbs.is_empty() {
        println!("  (no entity carries an Aabb — auto/absent)");
    }
    for (entity, name, bounds) in &aabbs {
        println!(
            "  {entity:?} \"{name}\": min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
            bounds[0], bounds[1], bounds[2], bounds[3], bounds[4], bounds[5]
        );
    }

    let mut auto_absent = 0usize;
    for (entity, name) in &mesh_entities {
        if world.get::<Aabb>(*entity).is_none() {
            auto_absent += 1;
            println!("  {entity:?} \"{name}\": auto/absent (mesh entity without Aabb)");
        }
    }
    println!(
        "  summary: {} with Aabb, {auto_absent} mesh entities auto/absent, \
         {} mesh entities total",
        aabbs.len(),
        mesh_entities.len()
    );
}

// ============================================================================
// Section 5: Cameras
// ============================================================================

fn print_cameras(app: &mut App) {
    println!("=== 5. Cameras ===");
    let world = app.world_mut();
    let mut query = world.query_filtered::<(Entity, Option<&Name>, &Camera, &Projection), ()>();
    let cameras: Vec<(Entity, String, bool, isize, Projection)> = query
        .iter(world)
        .map(|(entity, name, camera, projection)| {
            (
                entity,
                name_or(name, "(unnamed)"),
                camera.is_active,
                camera.order,
                projection.clone(),
            )
        })
        .collect();

    if cameras.is_empty() {
        println!("  (no cameras in the spawned scene)");
        return;
    }

    let mut active = 0usize;
    for (entity, name, is_active, order, projection) in &cameras {
        if *is_active {
            active += 1;
        }
        println!("  camera {entity:?} \"{name}\": is_active={is_active} order={order}");
        match projection {
            Projection::Perspective(p) => println!(
                "    perspective: fov={:.4} rad ({:.2} deg), near={:.4}, far={:.4}, \
                 aspect_ratio={:?}",
                p.fov,
                p.fov.to_degrees(),
                p.near,
                p.far,
                p.aspect_ratio
            ),
            Projection::Orthographic(o) => println!(
                "    orthographic: near={:.4}, far={:.4}, scaling_mode={:?}, scale={:.4}, \
                 viewport_origin={:?}",
                o.near, o.far, o.scaling_mode, o.scale, o.viewport_origin
            ),
            Projection::Custom(_) => println!("    custom projection"),
        }
    }
    println!(
        "  active: {active} of {} (loader activates the first camera in scene order; \
         exactly one active: {})",
        cameras.len(),
        yes_no(active == 1)
    );
}

// ============================================================================
// Section 6: Lights
// ============================================================================

fn print_lights(app: &mut App) {
    println!("=== 6. Lights ===");
    println!(
        "  (FBX light range via fbx_light_range: FarAttenuationEnd / AttenuationEnd / \
         DecayStart, fallback 20.0; SpotLight.radius mirrors range)"
    );
    let world = app.world_mut();

    let mut point_query = world.query_filtered::<(Entity, Option<&Name>, &PointLight), ()>();
    let points: Vec<(Entity, String, Color, f32, f32)> = point_query
        .iter(world)
        .map(|(e, n, l)| (e, name_or(n, "(unnamed)"), l.color, l.intensity, l.range))
        .collect();

    let mut spot_query = world.query_filtered::<(Entity, Option<&Name>, &SpotLight), ()>();
    let spots: Vec<(Entity, String, Color, f32, f32, f32, f32, f32)> = spot_query
        .iter(world)
        .map(|(e, n, l)| {
            (
                e,
                name_or(n, "(unnamed)"),
                l.color,
                l.intensity,
                l.range,
                l.radius,
                l.inner_angle,
                l.outer_angle,
            )
        })
        .collect();

    let mut dir_query = world.query_filtered::<(Entity, Option<&Name>, &DirectionalLight), ()>();
    let directionals: Vec<(Entity, String, Color, f32)> = dir_query
        .iter(world)
        .map(|(e, n, l)| (e, name_or(n, "(unnamed)"), l.color, l.illuminance))
        .collect();

    for (entity, name, color, intensity, range) in &points {
        println!(
            "  point       {entity:?} \"{name}\": range={range:.4} intensity={intensity:.4} \
             color={color:?}"
        );
    }
    for (entity, name, color, intensity, range, radius, inner, outer) in &spots {
        println!(
            "  spot        {entity:?} \"{name}\": range={range:.4} radius={radius:.4} \
             inner_angle={inner:.4} outer_angle={outer:.4} intensity={intensity:.4} \
             color={color:?}"
        );
    }
    for (entity, name, color, illuminance) in &directionals {
        println!(
            "  directional {entity:?} \"{name}\": illuminance={illuminance:.4} (no range) \
             color={color:?}"
        );
    }
    if points.is_empty() && spots.is_empty() && directionals.is_empty() {
        println!("  (no lights in the spawned scene)");
    } else {
        println!(
            "  total: {} point, {} spot, {} directional",
            points.len(),
            spots.len(),
            directionals.len()
        );
    }
}

// ============================================================================
// Section 7: StandardMaterial scalars + textures
// ============================================================================

fn print_materials(app: &App, handle: &Handle<Fbx>) {
    println!("=== 7. StandardMaterial scalars + textures ===");
    let world = app.world();
    let fbx = fbx_ref(world, handle);
    let fbx_materials = world.resource::<Assets<FbxMaterial>>();
    let materials = world.resource::<Assets<StandardMaterial>>();

    if fbx.materials.is_empty() {
        println!("  (no FbxMaterial entries)");
    }
    for (i, material_handle) in fbx.materials.iter().enumerate() {
        let Some(fbx_material) = fbx_materials.get(material_handle) else {
            println!("  materials[{i}]: (missing FbxMaterial asset)");
            continue;
        };
        let Some(mat) = materials.get(&fbx_material.material) else {
            println!(
                "  materials[{i}] \"{}\": (missing StandardMaterial asset)",
                fbx_material.name
            );
            continue;
        };
        println!("  materials[{i}] \"{}\":", fbx_material.name);
        println!("    base_color={:?}", mat.base_color);
        println!(
            "    base_color_texture={}",
            yes_no(mat.base_color_texture.is_some())
        );
        // ufbx `Texture::uv_transform` (translation/scale/rotation) conjugated
        // into the loader's flipped-V UV space (`F ∘ T ∘ F`), so an identity
        // authored transform prints the identity — see section 10 for a fixture
        // whose FBX texture carries a non-identity `Scaling`.
        println!("    uv_transform={:?}", mat.uv_transform);
        println!(
            "    metallic={:.4} perceptual_roughness={:.4} reflectance={:.4}",
            mat.metallic, mat.perceptual_roughness, mat.reflectance
        );
        println!(
            "    emissive={:?} emissive_texture={}",
            mat.emissive,
            yes_no(mat.emissive_texture.is_some())
        );
        println!(
            "    anisotropy_strength={:.4} anisotropy_rotation={:.4}",
            mat.anisotropy_strength, mat.anisotropy_rotation
        );
        println!(
            "    clearcoat={:.4} clearcoat_perceptual_roughness={:.4}",
            mat.clearcoat, mat.clearcoat_perceptual_roughness
        );
        println!(
            "    textures: normal={} metallic_roughness={} occlusion={}",
            yes_no(mat.normal_map_texture.is_some()),
            yes_no(mat.metallic_roughness_texture.is_some()),
            yes_no(mat.occlusion_texture.is_some())
        );
        #[cfg(feature = "pbr_multi_layer_material_textures")]
        {
            println!(
                "    clearcoat textures: strength={} roughness={} normal={} \
                 (pbr_multi_layer_material_textures)",
                yes_no(mat.clearcoat_texture.is_some()),
                yes_no(mat.clearcoat_roughness_texture.is_some()),
                yes_no(mat.clearcoat_normal_texture.is_some())
            );
        }
    }

    let total = materials.iter().count();
    let wrapped = fbx.materials.len();
    println!(
        "  StandardMaterial assets: {total} total, {wrapped} wrapped in FbxMaterial, \
         {} unwrapped (cull-inverted twins / DefaultMaterial)",
        total.saturating_sub(wrapped)
    );
}

// ============================================================================
// Section 8: Extras blobs
// ============================================================================

fn print_extras(app: &mut App, handle: &Handle<Fbx>) {
    println!("=== 8. Extras blobs ===");
    {
        let world = app.world();
        let fbx = fbx_ref(world, handle);
        let fbx_meshes = world.resource::<Assets<FbxMesh>>();
        let fbx_materials = world.resource::<Assets<FbxMaterial>>();
        let fbx_skins = world.resource::<Assets<FbxSkin>>();

        if fbx.meshes.is_empty() && fbx.materials.is_empty() && fbx.skins.is_empty() {
            println!("  (no meshes/materials/skins)");
        }
        for (i, mesh_handle) in fbx.meshes.iter().enumerate() {
            let Some(mesh) = fbx_meshes.get(mesh_handle) else {
                continue;
            };
            println!(
                "  FbxMesh[{i}] \"{}\": {}",
                mesh.name,
                extras_display(mesh.extras.as_ref().map(|e| e.value.as_str()))
            );
            for (p, prim) in mesh.primitives.iter().enumerate() {
                println!(
                    "    FbxPrimitive[{i}/{p}]: {}",
                    extras_display(prim.extras.as_ref().map(|e| e.value.as_str()))
                );
            }
        }
        for (i, material_handle) in fbx.materials.iter().enumerate() {
            let Some(material) = fbx_materials.get(material_handle) else {
                continue;
            };
            println!(
                "  FbxMaterial[{i}] \"{}\": {}",
                material.name,
                extras_display(material.extras.as_ref().map(|e| e.value.as_str()))
            );
        }
        for (i, skin_handle) in fbx.skins.iter().enumerate() {
            let Some(skin) = fbx_skins.get(skin_handle) else {
                continue;
            };
            println!(
                "  FbxSkin[{i}] \"{}\": {}",
                skin.name,
                extras_display(skin.extras.as_ref().map(|e| e.value.as_str()))
            );
        }
    }

    // `FbxNode` has no extras field (glTF `GltfNode::extras` parity gap): node
    // custom-property blobs arrive as `FbxExtras` components on spawned entities.
    println!("  spawned FbxExtras components (node/entity-level blobs):");
    let world = app.world_mut();
    let mut query = world.query_filtered::<(Entity, Option<&Name>, &FbxExtras), ()>();
    let entries: Vec<(Entity, String, String)> = query
        .iter(world)
        .map(|(e, n, x)| (e, name_or(n, "(unnamed)"), x.value.clone()))
        .collect();
    if entries.is_empty() {
        println!("    (none)");
    }
    for (entity, name, value) in entries {
        println!(
            "    {entity:?} \"{name}\": {}",
            extras_display(Some(value.as_str()))
        );
    }
}

// ============================================================================
// Section 9: Error display (granular FbxError on load failure)
// ============================================================================

fn print_error_section(path: &str, err: &AssetLoadError) {
    println!("=== 9. Error display ===");
    println!("failed to load '{path}'");

    let AssetLoadError::AssetLoaderError(loader_err) = err else {
        println!("  AssetLoadError (non-loader failure): {err}");
        return;
    };
    let Some(fbx_err) = loader_err.error().downcast_ref::<FbxError>() else {
        println!(
            "  AssetLoaderError for '{}' (not an FbxError): {}",
            loader_err.path(),
            loader_err.error()
        );
        return;
    };

    println!("  FbxError (Debug): {fbx_err:?}");
    match fbx_err {
        FbxError::Io(source) => {
            println!("    Io: {source}");
        }
        FbxError::UfbxError(message) => {
            println!("    UfbxError: {message}");
        }
        FbxError::UfbxLoad { path, message } => {
            println!("    UfbxLoad: path={path:?} message={message:?}");
        }
        FbxError::InvalidData(message) => {
            println!("    InvalidData: {message:?}");
        }
        FbxError::ConversionError(message) => {
            println!("    ConversionError: {message:?}");
        }
        FbxError::MeshConversion(message) => {
            println!("    MeshConversion: {message:?}");
        }
        FbxError::MorphTargets { mesh, message } => {
            println!("    MorphTargets: mesh={mesh:?} message={message:?}");
        }
        FbxError::MaterialConversion(message) => {
            println!("    MaterialConversion: {message:?}");
        }
        FbxError::TextureLoad(message) => {
            println!("    TextureLoad: {message:?}");
        }
        FbxError::UnsupportedFeature(message) => {
            println!("    UnsupportedFeature: {message:?}");
        }
        other => {
            // `FbxError` is `#[non_exhaustive]`: future variants land here.
            println!("    (other FbxError variant): {other}");
        }
    }
}

// ============================================================================
// Section 10: FbxLoaderSettings matrix (one knob flipped per load)
// ============================================================================
//
// bevy_asset keys a load by the full AssetPath (source + path + label) and
// returns the already-loaded handle for any later load of that path, so the
// FIRST settings applied to a path win. Every variant below therefore loads
// through its own registered `v_*://` asset source over the same `assets/`
// bytes — one (fixture, settings) pair per source, independent loader runs.

/// Asset sources registered in `main()` before `AssetPlugin` builds the
/// `AssetServer`; one per variant load of section 10.
const MATRIX_VARIANT_SOURCES: [&str; 16] = [
    "v_nocam",
    "v_nolight",
    "v_noanim",
    "v_rest",
    "v_fps",
    "v_fps_morph",
    "v_fps_multi",
    "v_native",
    "v_modify",
    "v_adjust",
    "v_troot",
    "v_bind",
    "v_nofrustum",
    "v_meshusage",
    "v_nomat",
    "v_include",
];

const FIX_CAMERA_LIGHT: &str = "maya_camera_light_axes_y_up_6100_binary.fbx";
const FIX_LIGHTS: &str = "motionbuilder_lights_7700_ascii.fbx";
const FIX_ANIM: &str = "cube_anim.fbx";
const FIX_MORPH: &str = "blend_shape_cube.fbx";
const FIX_SKIN: &str = "blender_279_sausage_7400_binary.fbx";
const FIX_MESHES: &str = "blender_suzanne_multimaterial_7400_binary.fbx";
const FIX_SOURCE: &str = "cube.fbx";
/// Single animation stack with THREE anim layers (BaseLayer/X/Y) — ufbx
/// `anim.layers.len() > 1` disables the exact-cubic path, so this take's
/// tracks come from ufbx's baked resample where `bake_fps` (resample_rate)
/// applies. Corpus file, absolute path.
const FIX_MULTILAYER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../libs/ufbx/data/maya_anim_layers_over_7500_ascii.fbx"
);
/// ufbx corpus fixture whose DiffuseColor `Texture::file3` carries a
/// non-identity `Scaling 2,2,1` FBX property (see `print_uv_transform_evidence`).
const FIX_UV_TRANSFORM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../libs/ufbx/data/maya_duplicated_texture_7700_ascii.fbx"
);

/// Everything section 10 measures about one (fixture, settings) load.
#[derive(Default)]
struct MatrixRow {
    cameras: usize,
    points: usize,
    spots: usize,
    directionals: usize,
    dynamic_bounds: usize,
    no_frustum_bounds: usize,
    animations: usize,
    named_animations: Vec<String>,
    clip_stats: Vec<String>,
    clip0_keys: Option<usize>,
    clip0_curves: usize,
    clip0_duration: f32,
    materials: usize,
    primitives: usize,
    mesh_usage: Vec<String>,
    source_bytes: Option<usize>,
    axis_unit: String,
    wrapper: String,
    roots: Vec<String>,
    aabb_count: usize,
    aabbs: Vec<String>,
}

fn matrix_baseline(app: &App, fixture: &str) -> Handle<Fbx> {
    app.world()
        .resource::<AssetServer>()
        .load(fixture.to_string())
}

#[allow(clippy::too_many_arguments)]
fn matrix_variant(
    app: &App,
    source: &str,
    fixture: &str,
    tweak: impl Fn(&mut FbxLoaderSettings) + Send + Sync + 'static,
) -> Handle<Fbx> {
    app.world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(tweak)
        .load(format!("{source}://{fixture}"))
}

/// Wait for one matrix load, then snapshot everything measurable about it.
fn waited_snapshot(app: &mut App, handle: &Handle<Fbx>) -> (LoadState, Option<MatrixRow>) {
    let state = wait_for_asset(app, handle, LOAD_TIMEOUT_FRAMES);
    let row = if matches!(state, LoadState::Loaded) {
        matrix_snapshot(app, handle)
    } else {
        None
    };
    (state, row)
}

fn matrix_snapshot(app: &App, handle: &Handle<Fbx>) -> Option<MatrixRow> {
    let world = app.world();
    let fbx = world.resource::<Assets<Fbx>>().get(handle)?;
    let mut row = MatrixRow {
        animations: fbx.animations.len(),
        materials: fbx.materials.len(),
        primitives: fbx.primitive_meshes.len(),
        source_bytes: fbx.source.as_ref().map(|bytes| bytes.len()),
        axis_unit: format!(
            "up=({:.3}, {:.3}, {:.3}) front=({:.3}, {:.3}, {:.3}) {:?} unit_scale={:.6}",
            fbx.axis_system.up.x,
            fbx.axis_system.up.y,
            fbx.axis_system.up.z,
            fbx.axis_system.front.x,
            fbx.axis_system.front.y,
            fbx.axis_system.front.z,
            fbx.axis_system.handedness,
            fbx.unit_scale,
        ),
        ..Default::default()
    };

    row.named_animations = fbx.named_animations.keys().map(|k| k.to_string()).collect();
    row.named_animations.sort();

    let clips = world.resource::<Assets<AnimationClip>>();
    for clip_handle in &fbx.animations {
        match clips.get(clip_handle) {
            Some(clip) => {
                let (known, unknown) = count_clip_keys(clip);
                let keys = if unknown > 0 {
                    format!("{known}+{unknown}?")
                } else {
                    known.to_string()
                };
                row.clip_stats.push(format!(
                    "duration={:.4}s curves={} keys={keys}",
                    clip.duration(),
                    clip.curves().len(),
                ));
                if row.clip0_keys.is_none() {
                    row.clip0_keys = (unknown == 0).then_some(known);
                    row.clip0_curves = clip.curves().len();
                    row.clip0_duration = clip.duration();
                }
            }
            None => row.clip_stats.push("(missing AnimationClip)".to_string()),
        }
    }

    let meshes = world.resource::<Assets<Mesh>>();
    row.mesh_usage = fbx
        .primitive_meshes
        .iter()
        .filter_map(|h| meshes.get(h))
        .map(|m| format!("{:?}", m.asset_usage))
        .collect();
    row.mesh_usage.sort();
    row.mesh_usage.dedup();

    // Counters come from the loader-built scene world itself (no spawn): the
    // same place tests/parity_contract.rs reads components from.
    let worlds = world.resource::<Assets<WorldAsset>>();
    if let Some(scene_handle) = fbx.default_scene.as_ref()
        && let Some(scene) = worlds.get(scene_handle)
    {
        let sw = &scene.world;
        let ids: Vec<Entity> = sw.iter_entities().map(|e| e.id()).collect();
        let wrapper = ids.iter().copied().find(|id| {
            sw.get::<Name>(*id)
                .is_some_and(|n| n.as_str() == "FbxSceneRoot")
        });
        row.wrapper = match wrapper {
            None => "absent".to_string(),
            Some(w) => match sw.get::<Transform>(w) {
                Some(t) => format!(
                    "present t={:?} r={:?} s={:?}",
                    t.translation, t.rotation, t.scale
                ),
                None => "present".to_string(),
            },
        };
        for id in ids {
            if Some(id) == wrapper {
                continue;
            }
            if sw.get::<Camera>(id).is_some() {
                row.cameras += 1;
            }
            if sw.get::<PointLight>(id).is_some() {
                row.points += 1;
            }
            if sw.get::<SpotLight>(id).is_some() {
                row.spots += 1;
            }
            if sw.get::<DirectionalLight>(id).is_some() {
                row.directionals += 1;
            }
            if sw.get::<DynamicSkinnedMeshBounds>(id).is_some() {
                row.dynamic_bounds += 1;
            }
            if sw.get::<NoFrustumCulling>(id).is_some() {
                row.no_frustum_bounds += 1;
            }
            if let Some(aabb) = sw.get::<Aabb>(id) {
                row.aabb_count += 1;
                if row.aabbs.len() < 3 {
                    let name = sw.get::<Name>(id).map_or("(unnamed)", |n| n.as_str());
                    row.aabbs.push(format!(
                        "\"{name}\" min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
                        aabb.min().x,
                        aabb.min().y,
                        aabb.min().z,
                        aabb.max().x,
                        aabb.max().y,
                        aabb.max().z,
                    ));
                }
            }
            // First-level scene nodes (direct wrapper children, or top level
            // when no wrapper): their transforms are where unit/axis
            // space-conversion differences show up.
            let parent = sw.get::<ChildOf>(id).map(|child_of| child_of.parent());
            let first_level = match (wrapper, parent) {
                (Some(w), Some(p)) => p == w,
                (None, None) => true,
                _ => false,
            };
            if first_level && row.roots.len() < 4 {
                let name = sw.get::<Name>(id).map_or("(unnamed)", |n| n.as_str());
                let transform = sw.get::<Transform>(id).map_or_else(
                    || "(no Transform)".to_string(),
                    |t| format!("t={:?} s={:?}", t.translation, t.scale),
                );
                row.roots.push(format!("\"{name}\" {transform}"));
            }
        }
    }

    Some(row)
}

/// Key count of one curve, parsed from its `Debug` dump (the curve types
/// expose no public key-count API; `AnimationCurve` only offers `domain()`).
fn curve_key_count(curve: &dyn AnimationCurve) -> Option<usize> {
    let dump = format!("{curve:?}");
    // The rest clip's single-value curves carry no key-time array; one constant
    // value is one "key" (docs: rest clip = single keyframe at t = 0).
    if dump.contains("ConstantCurve") {
        return Some(1);
    }
    for pattern in [
        "keyframe_timestamps: [",
        "timestamps: [",
        "key_times: [",
        "times: [",
    ] {
        if let Some(pos) = dump.find(pattern) {
            let rest = &dump[pos + pattern.len()..];
            let end = rest.find(']')?;
            let inner = rest[..end].trim();
            return Some(if inner.is_empty() {
                0
            } else {
                inner.split(',').count()
            });
        }
    }
    None
}

fn count_clip_keys(clip: &AnimationClip) -> (usize, usize) {
    let mut known = 0usize;
    let mut unknown = 0usize;
    for curves in clip.curves().values() {
        for curve in curves {
            match curve_key_count(&*curve.0) {
                Some(keys) => known += keys,
                None => unknown += 1,
            }
        }
    }
    (known, unknown)
}

/// Sample every curve of a clip on a fixed 17-point grid over its duration.
/// Curves whose sampled type is not one of the known scalar/vector types are
/// recorded as empty (skipped by [`probe_delta`]).
fn clip_samples(clip: &AnimationClip) -> HashMap<(String, usize), Vec<Vec<f32>>> {
    let duration = clip.duration().max(1e-4);
    let mut out: HashMap<(String, usize), Vec<Vec<f32>>> = HashMap::new();
    for (target, curves) in clip.curves() {
        for (index, curve) in curves.iter().enumerate() {
            let mut samples = Vec::with_capacity(17);
            for step in 0..=16u32 {
                let t = duration * step as f32 / 16.0;
                match sample_f32s(&*curve.0, t) {
                    Some(values) => samples.push(values),
                    None => {
                        samples.clear();
                        break;
                    }
                }
            }
            out.insert((format!("{target:?}"), index), samples);
        }
    }
    out
}

fn sample_f32s(curve: &dyn AnimationCurve, t: f32) -> Option<Vec<f32>> {
    let value = curve.sample_clamped(t);
    if let Some(v) = value.downcast_ref::<f32>() {
        Some(vec![*v])
    } else if let Some(v) = value.downcast_ref::<Vec3>() {
        Some(vec![v.x, v.y, v.z])
    } else if let Some(v) = value.downcast_ref::<Quat>() {
        Some(vec![v.x, v.y, v.z, v.w])
    } else {
        None
    }
}

/// Largest absolute value difference between the two loads' clip-0 curves,
/// sampled at 17 probe times: measures whether a knob (e.g. `bake_fps`)
/// changed actual curve values, not just metadata.
fn probe_delta(app: &App, base: &Handle<Fbx>, variant: &Handle<Fbx>) -> String {
    let world = app.world();
    let fbx_assets = world.resource::<Assets<Fbx>>();
    let clips = world.resource::<Assets<AnimationClip>>();
    let (Some(base_fbx), Some(variant_fbx)) = (fbx_assets.get(base), fbx_assets.get(variant))
    else {
        return "n/a (Fbx asset missing)".to_string();
    };
    let (Some(base_clip), Some(variant_clip)) = (
        base_fbx.animations.first().and_then(|h| clips.get(h)),
        variant_fbx.animations.first().and_then(|h| clips.get(h)),
    ) else {
        return "n/a (no clip 0)".to_string();
    };

    let base_samples = clip_samples(base_clip);
    let variant_samples = clip_samples(variant_clip);
    if base_samples.len() != variant_samples.len() {
        return format!(
            "curve sets differ ({} vs {} curves)",
            base_samples.len(),
            variant_samples.len()
        );
    }
    let mut max_diff = 0.0f32;
    let mut compared = 0usize;
    let mut unsupported = 0usize;
    for (key, base_values) in &base_samples {
        let Some(variant_values) = variant_samples.get(key) else {
            return "curve sets differ (key missing)".to_string();
        };
        if base_values.is_empty() || variant_values.is_empty() {
            unsupported += 1;
            continue;
        }
        if base_values.len() != variant_values.len() {
            return format!("sample arity differs at {key:?}");
        }
        for (base_sample, variant_sample) in base_values.iter().zip(variant_values) {
            if base_sample.len() != variant_sample.len() {
                return format!("value arity differs at {key:?}");
            }
            for (x, y) in base_sample.iter().zip(variant_sample) {
                max_diff = max_diff.max((x - y).abs());
                compared += 1;
            }
        }
    }
    if compared == 0 {
        format!("no comparable samples ({unsupported} curves of unsupported types)")
    } else {
        format!(
            "max |Δ| = {max_diff:.6} over {compared} samples ({} curves, \
             {unsupported} unsupported)",
            base_samples.len()
        )
    }
}

fn row_line(
    state: &LoadState,
    row: &Option<MatrixRow>,
    describe: &impl Fn(&MatrixRow) -> String,
) -> String {
    match row {
        Some(row) => describe(row),
        None => format!("[load {}]", state_str(state)),
    }
}

#[allow(clippy::too_many_arguments)]
fn print_matrix_row(
    knob: &str,
    fixture: &str,
    base_state: &LoadState,
    base_row: &Option<MatrixRow>,
    variant_state: &LoadState,
    variant_row: &Option<MatrixRow>,
    describe: impl Fn(&MatrixRow) -> String,
    delta: impl Fn(&MatrixRow, &MatrixRow) -> String,
) {
    println!("  {knob} @ {fixture}");
    println!("    default: {}", row_line(base_state, base_row, &describe));
    println!(
        "    variant: {}",
        row_line(variant_state, variant_row, &describe)
    );
    match (base_row, variant_row) {
        (Some(base), Some(variant)) => println!("    delta:   {}", delta(base, variant)),
        _ => println!("    delta:   (unavailable — a load failed)"),
    }
}

fn print_settings_matrix(app: &mut App) {
    println!("=== 10. FbxLoaderSettings matrix ===");
    println!("  same fixture bytes, one knob flipped per load; every variant goes through its own");
    println!("  v_*:// asset source (bevy_asset dedups loads by AssetPath — first settings win)");

    // Issue all loads up front so they progress concurrently while we wait on
    // each one in turn.
    let cam_base = matrix_baseline(app, FIX_CAMERA_LIGHT);
    let cam_variant = matrix_variant(app, "v_nocam", FIX_CAMERA_LIGHT, |s| {
        s.load_cameras = false;
    });
    let lights_base = matrix_baseline(app, FIX_LIGHTS);
    let lights_variant = matrix_variant(app, "v_nolight", FIX_LIGHTS, |s| {
        s.load_lights = false;
    });
    let anim_base = matrix_baseline(app, FIX_ANIM);
    let anim_noanim = matrix_variant(app, "v_noanim", FIX_ANIM, |s| {
        s.load_animations = false;
    });
    let anim_rest = matrix_variant(app, "v_rest", FIX_ANIM, |s| {
        s.generate_rest_animation = true;
    });
    let anim_fps = matrix_variant(app, "v_fps", FIX_ANIM, |s| s.bake_fps = 120.0);
    let anim_native = matrix_variant(app, "v_native", FIX_ANIM, |s| {
        s.convert_coordinates = false;
    });
    let anim_modify = matrix_variant(app, "v_modify", FIX_ANIM, |s| {
        s.space_conversion = FbxSpaceConversion::ModifyGeometry;
    });
    let anim_adjust = matrix_variant(app, "v_adjust", FIX_ANIM, |s| {
        s.space_conversion = FbxSpaceConversion::AdjustTransforms;
    });
    let anim_troot = matrix_variant(app, "v_troot", FIX_ANIM, |s| {
        s.space_conversion = FbxSpaceConversion::TransformRoot;
    });
    let morph_base = matrix_baseline(app, FIX_MORPH);
    let morph_fps = matrix_variant(app, "v_fps_morph", FIX_MORPH, |s| s.bake_fps = 120.0);
    let skin_base = matrix_baseline(app, FIX_SKIN);
    let skin_bind = matrix_variant(app, "v_bind", FIX_SKIN, |s| {
        s.skinned_mesh_bounds_policy = FbxSkinnedMeshBoundsPolicy::BindPose;
    });
    let skin_nofrustum = matrix_variant(app, "v_nofrustum", FIX_SKIN, |s| {
        s.skinned_mesh_bounds_policy = FbxSkinnedMeshBoundsPolicy::NoFrustumCulling;
    });
    let mesh_base = matrix_baseline(app, FIX_MESHES);
    let mesh_usage = matrix_variant(app, "v_meshusage", FIX_MESHES, |s| {
        s.load_meshes = RenderAssetUsages::RENDER_WORLD;
    });
    let mesh_nomat = matrix_variant(app, "v_nomat", FIX_MESHES, |s| {
        s.load_materials = RenderAssetUsages::empty();
    });
    let source_base = matrix_baseline(app, FIX_SOURCE);
    let source_include = matrix_variant(app, "v_include", FIX_SOURCE, |s| {
        s.include_source = true;
    });
    let multi_base = matrix_baseline(app, FIX_MULTILAYER);
    let multi_fps = matrix_variant(app, "v_fps_multi", FIX_MULTILAYER, |s| s.bake_fps = 120.0);

    let cam_light_desc = |r: &MatrixRow| {
        format!(
            "cameras={} point={} spot={} dir={}",
            r.cameras, r.points, r.spots, r.directionals
        )
    };
    let (state, row) = waited_snapshot(app, &cam_base);
    let (v_state, v_row) = waited_snapshot(app, &cam_variant);
    print_matrix_row(
        "load_cameras=false",
        FIX_CAMERA_LIGHT,
        &state,
        &row,
        &v_state,
        &v_row,
        cam_light_desc,
        |b, v| {
            format!(
                "cameras {} -> {} ({:+}), point/spot/dir lights unchanged ({} -> {})",
                b.cameras,
                v.cameras,
                v.cameras as isize - b.cameras as isize,
                b.points + b.spots + b.directionals,
                v.points + v.spots + v.directionals,
            )
        },
    );

    let (state, row) = waited_snapshot(app, &lights_base);
    let (v_state, v_row) = waited_snapshot(app, &lights_variant);
    print_matrix_row(
        "load_lights=false",
        FIX_LIGHTS,
        &state,
        &row,
        &v_state,
        &v_row,
        cam_light_desc,
        |b, v| {
            let base_total = b.points + b.spots + b.directionals;
            let variant_total = v.points + v.spots + v.directionals;
            format!(
                "point {} -> {}, spot {} -> {}, dir {} -> {} (total {base_total} -> \
                 {variant_total}, {:+})",
                b.points,
                v.points,
                b.spots,
                v.spots,
                b.directionals,
                v.directionals,
                variant_total as isize - base_total as isize,
            )
        },
    );

    let anim_desc = |r: &MatrixRow| {
        format!(
            "animations={} named=[{}] clips=[{}]",
            r.animations,
            r.named_animations.join(", "),
            r.clip_stats.join(" | "),
        )
    };
    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_noanim);
    print_matrix_row(
        "load_animations=false",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        anim_desc,
        |b, v| {
            format!(
                "animations {} -> {} ({} -> {} named takes), clip assets {} -> {}",
                b.animations,
                v.animations,
                b.named_animations.len(),
                v.named_animations.len(),
                b.clip_stats.len(),
                v.clip_stats.len(),
            )
        },
    );

    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_rest);
    print_matrix_row(
        "generate_rest_animation=true",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        anim_desc,
        |b, v| {
            let rest = if v.named_animations.iter().any(|k| k == "Rest") {
                "\"Rest\" present"
            } else {
                "\"Rest\" MISSING"
            };
            format!(
                "animations {} -> {} (+rest clip), named +{rest}, clip stats: [{}]",
                b.animations,
                v.animations,
                v.clip_stats.join(" | "),
            )
        },
    );
    // Label-level evidence for the rest clip is printed AFTER every matrix row:
    // probing the absent `cube_anim.fbx#AnimationRest` label on the default
    // source fails, and a failed label load leaves that path's aggregate load
    // state stuck in `Loading` for later baseline waits.

    let keys_desc = |r: &MatrixRow| {
        format!(
            "clip0 duration={:.4}s curves={} keys={} (clip stats: [{}])",
            r.clip0_duration,
            r.clip0_curves,
            r.clip0_keys
                .map_or_else(|| "?".to_string(), |k| k.to_string()),
            r.clip_stats.first().cloned().unwrap_or_default(),
        )
    };
    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_fps);
    print_matrix_row(
        "bake_fps=120.0 (default 30.0)",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        keys_desc,
        |b, v| match (b.clip0_keys, v.clip0_keys) {
            (Some(base_keys), Some(variant_keys)) if base_keys == variant_keys => format!(
                "clip0 keys unchanged ({base_keys}) — exact-cubic path keeps authored key \
                 times; duration {:.4} -> {:.4}",
                b.clip0_duration, v.clip0_duration
            ),
            (Some(base_keys), Some(variant_keys)) => format!(
                "clip0 keys {base_keys} -> {variant_keys} ({:+}), duration {:.4} -> {:.4}",
                variant_keys as isize - base_keys as isize,
                b.clip0_duration,
                v.clip0_duration,
            ),
            _ => "clip0 key counts not parseable".to_string(),
        },
    );
    println!("    probe:   {}", probe_delta(app, &anim_base, &anim_fps));

    let (state, row) = waited_snapshot(app, &morph_base);
    let (v_state, v_row) = waited_snapshot(app, &morph_fps);
    print_matrix_row(
        "bake_fps=120.0 (morph fixture)",
        FIX_MORPH,
        &state,
        &row,
        &v_state,
        &v_row,
        keys_desc,
        |b, v| match (b.clip0_keys, v.clip0_keys) {
            (Some(base_keys), Some(variant_keys)) if base_keys == variant_keys => format!(
                "clip0 keys unchanged ({base_keys}) — morph/exact path keeps authored key \
                 times; duration {:.4} -> {:.4}",
                b.clip0_duration, v.clip0_duration
            ),
            (Some(base_keys), Some(variant_keys)) => format!(
                "clip0 keys {base_keys} -> {variant_keys} ({:+}) — morph fallback sample \
                 count = ceil(duration * bake_fps); duration {:.4} -> {:.4}",
                variant_keys as isize - base_keys as isize,
                b.clip0_duration,
                v.clip0_duration,
            ),
            _ => "clip0 key counts not parseable (mix of known/unknown curve types)".to_string(),
        },
    );
    println!("    probe:   {}", probe_delta(app, &morph_base, &morph_fps));

    let space_desc = |r: &MatrixRow| {
        format!(
            "axis[{}] wrapper={} roots=[{}] aabbs[{}]={:?}",
            r.axis_unit,
            r.wrapper,
            r.roots.join(" | "),
            r.aabb_count,
            r.aabbs,
        )
    };
    let space_delta = |b: &MatrixRow, v: &MatrixRow| {
        let mut changes = Vec::new();
        if b.axis_unit != v.axis_unit {
            changes.push(format!("axis/unit: [{}] -> [{}]", b.axis_unit, v.axis_unit));
        }
        if b.wrapper != v.wrapper {
            changes.push(format!("wrapper: {} -> {}", b.wrapper, v.wrapper));
        }
        if b.roots != v.roots {
            changes.push(format!(
                "roots: [{}] -> [{}]",
                b.roots.join(" | "),
                v.roots.join(" | "),
            ));
        }
        if b.aabbs != v.aabbs {
            changes.push(format!("aabbs: {:?} -> {:?}", b.aabbs, v.aabbs));
        }
        if changes.is_empty() {
            "no observable delta (authored space already equals the variant result)".to_string()
        } else {
            changes.join("; ")
        }
    };

    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_native);
    print_matrix_row(
        "convert_coordinates=false",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        space_desc,
        space_delta,
    );

    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_modify);
    print_matrix_row(
        "space_conversion=ModifyGeometry (default Auto)",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        space_desc,
        space_delta,
    );

    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_adjust);
    print_matrix_row(
        "space_conversion=AdjustTransforms (default Auto)",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        space_desc,
        space_delta,
    );

    let (state, row) = waited_snapshot(app, &anim_base);
    let (v_state, v_row) = waited_snapshot(app, &anim_troot);
    print_matrix_row(
        "space_conversion=TransformRoot (default Auto)",
        FIX_ANIM,
        &state,
        &row,
        &v_state,
        &v_row,
        space_desc,
        space_delta,
    );

    let bounds_desc = |r: &MatrixRow| {
        format!(
            "DynamicSkinnedMeshBounds={} NoFrustumCulling={} Aabb={} (scene entities: \
             cameras={} lights={})",
            r.dynamic_bounds,
            r.no_frustum_bounds,
            r.aabb_count,
            r.cameras,
            r.points + r.spots + r.directionals,
        )
    };
    let (state, row) = waited_snapshot(app, &skin_base);
    let (v_state, v_row) = waited_snapshot(app, &skin_bind);
    print_matrix_row(
        "skinned_mesh_bounds_policy=BindPose (default Dynamic)",
        FIX_SKIN,
        &state,
        &row,
        &v_state,
        &v_row,
        bounds_desc,
        |b, v| {
            format!(
                "DynamicSkinnedMeshBounds {} -> {}, NoFrustumCulling {} -> {} (BindPose \
                 inserts neither marker; Aabb still present)",
                b.dynamic_bounds, v.dynamic_bounds, b.no_frustum_bounds, v.no_frustum_bounds,
            )
        },
    );

    let (state, row) = waited_snapshot(app, &skin_base);
    let (v_state, v_row) = waited_snapshot(app, &skin_nofrustum);
    print_matrix_row(
        "skinned_mesh_bounds_policy=NoFrustumCulling (default Dynamic)",
        FIX_SKIN,
        &state,
        &row,
        &v_state,
        &v_row,
        bounds_desc,
        |b, v| {
            format!(
                "DynamicSkinnedMeshBounds {} -> {}, NoFrustumCulling {} -> {}",
                b.dynamic_bounds, v.dynamic_bounds, b.no_frustum_bounds, v.no_frustum_bounds,
            )
        },
    );

    let mesh_desc = |r: &MatrixRow| {
        format!(
            "primitives={} mesh.asset_usage=[{}] materials={}",
            r.primitives,
            r.mesh_usage.join(", "),
            r.materials,
        )
    };
    let (state, row) = waited_snapshot(app, &mesh_base);
    let (v_state, v_row) = waited_snapshot(app, &mesh_usage);
    print_matrix_row(
        "load_meshes=RenderAssetUsages::RENDER_WORLD (default MAIN_WORLD|RENDER_WORLD)",
        FIX_MESHES,
        &state,
        &row,
        &v_state,
        &v_row,
        mesh_desc,
        |b, v| {
            format!(
                "mesh.asset_usage [{}] -> [{}] ({} -> {} primitives, vertex data still in \
                 Assets<Mesh> headless — nothing extracts without a render world)",
                b.mesh_usage.join(", "),
                v.mesh_usage.join(", "),
                b.primitives,
                v.primitives,
            )
        },
    );

    let (state, row) = waited_snapshot(app, &mesh_base);
    let (v_state, v_row) = waited_snapshot(app, &mesh_nomat);
    print_matrix_row(
        "load_materials=RenderAssetUsages::empty() (default MAIN_WORLD|RENDER_WORLD)",
        FIX_MESHES,
        &state,
        &row,
        &v_state,
        &v_row,
        mesh_desc,
        |b, v| {
            format!(
                "materials {} -> {} (empty usage skips material processing entirely — \
                 FbxMaterial/StandardMaterial pools stay empty; primitives {} -> {})",
                b.materials, v.materials, b.primitives, v.primitives,
            )
        },
    );

    let source_desc = |r: &MatrixRow| match r.source_bytes {
        Some(bytes) => format!("fbx.source=Some([{bytes} bytes] — raw file bytes embedded)"),
        None => "fbx.source=None".to_string(),
    };
    let (state, row) = waited_snapshot(app, &source_base);
    let (v_state, v_row) = waited_snapshot(app, &source_include);
    print_matrix_row(
        "include_source=true",
        FIX_SOURCE,
        &state,
        &row,
        &v_state,
        &v_row,
        source_desc,
        |b, v| {
            format!(
                "fbx.source {} -> {} (file is {} bytes; nothing else about the load changes)",
                b.source_bytes
                    .map_or("None".to_string(), |n| format!("[{n}]")),
                v.source_bytes
                    .map_or("None".to_string(), |n| format!("[{n}]")),
                v.source_bytes.unwrap_or(0),
            )
        },
    );

    // 3-layer stack: ufbx's exact-cubic path only applies to single-layer
    // takes, so these tracks DO come from ufbx's baked resample (resample_rate
    // = bake_fps). Observed: ufbx key reduction converges to the same key set
    // at 30 and 120 for this take — measured, not assumed.
    let (state, row) = waited_snapshot(app, &multi_base);
    let (v_state, v_row) = waited_snapshot(app, &multi_fps);
    print_matrix_row(
        "bake_fps=120.0 (3-layer stack — baked fallback)",
        FIX_MULTILAYER,
        &state,
        &row,
        &v_state,
        &v_row,
        keys_desc,
        |b, v| match (b.clip0_keys, v.clip0_keys) {
            (Some(base_keys), Some(variant_keys)) if base_keys == variant_keys => format!(
                "clip0 keys unchanged ({base_keys}) — baked path (3 layers) but ufbx key \
                 reduction converged to the same key set at 30 vs 120; duration {:.4} -> {:.4}",
                b.clip0_duration, v.clip0_duration
            ),
            (Some(base_keys), Some(variant_keys)) => format!(
                "clip0 keys {base_keys} -> {variant_keys} ({:+}) — baked resample tracks \
                 key count follows duration*bake_fps; duration {:.4} -> {:.4}",
                variant_keys as isize - base_keys as isize,
                b.clip0_duration,
                v.clip0_duration,
            ),
            _ => "clip0 key counts not parseable (mix of known/unknown curve types)".to_string(),
        },
    );
    println!("    probe:   {}", probe_delta(app, &multi_base, &multi_fps));

    // Label-level evidence: AnimationRest exists only in the run with
    // generate_rest_animation=true. (Deliberately last: the default-source
    // probe fails, and a failed label load leaves that path's aggregate load
    // state stuck in `Loading` for any later wait.)
    for (tag, source_path) in [
        ("default", FIX_ANIM.to_string()),
        ("variant", format!("v_rest://{FIX_ANIM}")),
    ] {
        let label_path = FbxAssetLabel::AnimationRest.from_asset(source_path);
        let label_handle: Handle<AnimationClip> = app
            .world()
            .resource::<AssetServer>()
            .load(label_path.clone());
        let label_state = wait_for_asset(app, &label_handle, LABEL_TIMEOUT_FRAMES);
        println!(
            "  AnimationRest label[{tag}]: {label_path} [{}]",
            state_str(&label_state)
        );
    }

    print_uv_transform_evidence(app);
}

/// Non-identity `uv_transform` evidence. Hunt method: brace-matched every
/// `Texture:` block in the 396 ASCII `.fbx` files of `libs/ufbx/data` for
/// Translation/Rotation/Scaling/UVSwap/TextureSwap ≠ defaults, then followed
/// each hit's material connection — only `maya_duplicated_texture_7700_ascii`
/// lands on DiffuseColor (the base color slot, the only slot whose transform
/// reaches `StandardMaterial::uv_transform`).
fn print_uv_transform_evidence(app: &mut App) {
    println!("  uv_transform @ maya_duplicated_texture_7700_ascii.fbx (ufbx corpus):");
    println!(
        "    hunt: all 396 ASCII .fbx under libs/ufbx/data brace-scanned for non-identity \
         Texture Translation/Rotation/Scaling/UVSwap; this fixture's DiffuseColor \
         Texture::file3 carries Scaling 2,2,1 (the only other hit, maya_texture_layers_7500, \
         drives Emissive/Transparent slots which never reach StandardMaterial::uv_transform)"
    );
    let handle: Handle<Fbx> = app
        .world()
        .resource::<AssetServer>()
        .load(FIX_UV_TRANSFORM.to_string());
    let state = wait_for_asset(app, &handle, LOAD_TIMEOUT_FRAMES);
    if !matches!(state, LoadState::Loaded) {
        println!("    load: {} (evidence unavailable)", state_str(&state));
        return;
    }
    let world = app.world();
    let fbx = fbx_ref(world, &handle);
    let fbx_materials = world.resource::<Assets<FbxMaterial>>();
    let materials = world.resource::<Assets<StandardMaterial>>();
    if fbx.materials.is_empty() {
        println!("    (fixture produced no materials)");
    }
    for (i, material_handle) in fbx.materials.iter().enumerate() {
        let Some(fbx_material) = fbx_materials.get(material_handle) else {
            continue;
        };
        match materials.get(&fbx_material.material) {
            Some(mat) => println!(
                "    materials[{i}] \"{}\": uv_transform={:?} base_color_texture={}",
                fbx_material.name,
                mat.uv_transform,
                yes_no(mat.base_color_texture.is_some()),
            ),
            None => println!(
                "    materials[{i}] \"{}\": (missing StandardMaterial)",
                fbx_material.name
            ),
        }
    }
}
