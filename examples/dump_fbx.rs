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

use bevy::animation::{
    AnimatedBy, AnimationClip, AnimationPlayer, AnimationPlugin, AnimationTargetId,
};
use bevy::asset::{AssetLoadError, AssetPlugin, LoadState, UnapprovedPathMode};
use bevy::camera::primitives::Aabb;
use bevy::image::Image;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldSerializationPlugin};
use bevy_ufbx::{
    Fbx, FbxAssetLabel, FbxError, FbxExtras, FbxMaterial, FbxMesh, FbxNode, FbxPlugin,
    FbxSceneExtras, FbxSceneName, FbxSkin,
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
