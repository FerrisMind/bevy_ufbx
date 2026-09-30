//! Static mesh + `FbxAssetLabel::Mesh`/`Primitive` resolution + unit conversion.
//!
//! ```sh
//! cargo run --example static_mesh_fbx
//! ```
//!
//! Prefers Blender Suzanne (`AdjustTransforms`); falls back to `cube.fbx`, then
//! `maya_cube_7400_binary.fbx` (the 1 cm Maya fixture gets a 100x demo scale).
//! Loads `Scene0` **and** the `Mesh0` / `Mesh0/Primitive0` labels of the same
//! file, then proves on screen that the `Primitive0` handle is the very handle
//! `Scene0` renders. After spawn the camera auto-frames the measured content
//! `Aabb` (spin-invariant sphere around the Y axis — see `frame_camera`), so
//! whichever fallback asset was picked lands on screen; the old hard-coded
//! camera sat *inside* the ×100 Maya fixture. `cube.fbx` ships an FBX camera
//! that `Scene0` activates (glTF parity) — this demo deactivates it so its own
//! framing camera wins.
//! Look for: recognizable spinning mesh + the on-screen label report.

use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::{
    asset::LoadState,
    camera::primitives::Aabb,
    light::{CascadeShadowConfig, CascadeShadowConfigBuilder},
    prelude::*,
    window::PrimaryWindow,
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxMesh, FbxPlugin};

const SUZANNE: &str = "blender_282_suzanne_7400_binary.fbx";
const CUBE: &str = "cube.fbx";
const MAYA_CUBE: &str = "maya_cube_7400_binary.fbx";
/// The Maya cube converts to a 1 cm fixture (`Aabb ±0.005` world units, see
/// `dump_fbx maya_cube_7400_binary.fbx`); ×100 makes it a visible ~1 m cube.
/// Suzanne and `cube.fbx` are already metre-scale (±1.37 / ±1.0) — they keep
/// scale 1.0 (the old blanket ×100 put the camera inside the 2 m cube).
const CM_DEMO_SCALE: f32 = 100.0;

/// Frames to poll for mesh `Aabb`s before keeping the default framing
/// (`Aabb` shows up a frame after the scene entities spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Frustum margin on the fitted distance (bounding sphere × slack).
const FRAMING_MARGIN: f32 = 1.15;
/// Viewpoint direction of the original hard-coded camera — the auto-fit
/// camera sits at the same diagonal angle, just at the measured distance.
const VIEW_DIR: Vec3 = Vec3::new(3.0, 2.2, 4.5);

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — static_mesh_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, report_labels, frame_camera))
        .run();
}

#[derive(Component)]
struct Spinning;

#[derive(Component)]
struct ViewerCamera;

/// Demo ground plane — excluded from framing bounds, repositioned under the
/// measured content by `frame_camera`.
#[derive(Component)]
struct Ground;

#[derive(Component)]
struct StatusText;

/// Fit-to-content state machine (the `load_fbx` pattern): armed by the
/// `WorldInstanceReady` observer, polled until `Aabb`s exist.
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

/// The three label loads this example proves, plus the one-shot report flag.
#[derive(Resource)]
struct LabelReport {
    path: String,
    scale: f32,
    fbx: Handle<Fbx>,
    mesh: Handle<FbxMesh>,
    primitive: Handle<Mesh>,
    reported: bool,
}

fn pick_fbx() -> (&'static str, f32) {
    let assets = Path::new("assets");
    if assets.join(SUZANNE).is_file() {
        (SUZANNE, 1.0)
    } else if assets.join(CUBE).is_file() {
        (CUBE, 1.0)
    } else {
        (MAYA_CUBE, CM_DEMO_SCALE)
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (fbx_path, scale) = pick_fbx();
    assert!(
        Path::new("assets").join(fbx_path).is_file(),
        "missing assets/{fbx_path}"
    );

    commands
        .spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(fbx_path))),
            Transform::from_scale(Vec3::splat(scale)),
            Spinning,
        ))
        .observe(scene_ready);

    // Same file through the Fbx index + the two mesh labels — `report_labels`
    // verifies on screen that `Mesh0/Primitive0` IS the rendered geometry.
    commands.insert_resource(LabelReport {
        path: fbx_path.to_string(),
        scale,
        fbx: asset_server.load(fbx_path),
        mesh: asset_server.load(FbxAssetLabel::Mesh(0).from_asset(fbx_path)),
        primitive: asset_server.load(
            FbxAssetLabel::Primitive {
                mesh: 0,
                primitive: 0,
            }
            .from_asset(fbx_path),
        ),
        reported: false,
    });

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        // near=0.01 (Bevy default 0.1) lets the fit-to-content framing get
        // close to the 1 cm fixture; Bevy's reverse-infinite-Z projection
        // keeps depth precision fine at this near/far ratio.
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_translation(VIEW_DIR * 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Ground,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.1, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 20.0,
            maximum_distance: 40.0,
            ..default()
        }
        .build(),
    ));

    commands.spawn((
        StatusText,
        Text::new(format!(
            "static_mesh_fbx — {fbx_path}\n\
             Look for: spinning mesh + on-screen label-resolution report.\n\
             Waiting for Scene0 + Mesh0/Primitive0 labels..."
        )),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Scene instance spawned → arm the fit-to-content framing and deactivate any
/// camera that came from the FBX file (`cube.fbx`'s `CINEMA_4D_Editor`
/// activates as the scene's camera — glTF parity — which would fight this
/// demo's own viewer camera).
fn scene_ready(
    _ready: On<WorldInstanceReady>,
    mut framing: ResMut<CameraFraming>,
    root: Query<Entity, With<WorldAssetRoot>>,
    children: Query<&Children>,
    mut cameras: Query<&mut Camera>,
) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
    if let Ok(root) = root.single() {
        for child in children.iter_descendants(root) {
            if let Ok(mut cam) = cameras.get_mut(child)
                && cam.is_active
            {
                cam.is_active = false;
                info!("static_mesh_fbx: deactivated FBX-imported camera (this demo owns the view)");
            }
        }
    }
}

/// Fit the camera to the measured content bounds (the `load_fbx` pattern).
///
/// Every example transform spins the model about the Y axis through the
/// origin, so the frame is a **spin-invariant** bounding sphere: horizontal
/// distance to that axis and height are rotation-proof, unlike the raw union
/// `Aabb` which changes as the model turns. The ground plane and the shadow
/// cascades are re-fitted from the same measurement (the ×100 Maya fixture is
/// far beyond the old fixed 10-unit plane / 20-unit cascade bound).
fn frame_camera(
    mut framing: ResMut<CameraFraming>,
    // Camera and ground both write `Transform`; `With<ViewerCamera>` /
    // `With<Ground>` do not prove disjointness (B0001), so they share a
    // ParamSet.
    mut rigs: ParamSet<(
        Query<(&mut Transform, &Projection), With<ViewerCamera>>,
        Query<&mut Transform, With<Ground>>,
    )>,
    meshes: Query<(&Aabb, &GlobalTransform), (With<Mesh3d>, Without<Ground>)>,
    mut cascades: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;

    // Union of world-space mesh bounds (ground excluded — it is dressing, not
    // content). `Aabb` is inserted by Bevy's calculate_bounds after spawn.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut corners: Vec<Vec3> = Vec::new();
    let mut count = 0usize;
    for (aabb, global) in meshes.iter() {
        let (lo, hi) = (aabb.min(), aabb.max());
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    min = min.min(world);
                    max = max.max(world);
                    corners.push(world);
                }
            }
        }
        count += 1;
    }
    if count == 0 {
        if framing.frames >= FRAMING_TIMEOUT_FRAMES {
            framing.armed = false;
            framing.done = true;
            println!(
                "[static_mesh_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
                 keeping default camera framing"
            );
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    let center = Vec3::new(0.0, (min.y + max.y) * 0.5, 0.0);
    let mut radius = (max.y - min.y).abs() * 0.5;
    for world in &corners {
        radius = radius.max((*world - center).length());
    }
    radius = radius.max(1e-4);

    {
        let mut ground_q = rigs.p1();
        if let Ok(mut ground) = ground_q.single_mut() {
            ground.translation.y = min.y - 0.06 * radius;
            ground.scale = Vec3::splat((radius * 0.6).max(1.0));
        }
    }
    if let Ok(mut cascade) = cascades.single_mut() {
        *cascade = CascadeShadowConfigBuilder {
            first_cascade_far_bound: (radius * 4.0).max(1.0),
            maximum_distance: (radius * 8.0).max(2.0),
            ..default()
        }
        .build();
    }

    let mut camera_q = rigs.p0();
    let Ok((mut cam_tf, projection)) = camera_q.single_mut() else {
        return;
    };
    let (v_fov, near) = match projection {
        Projection::Perspective(p) => (p.fov, p.near),
        _ => (FRAC_PI_4, 0.1),
    };
    let aspect = windows
        .single()
        .map(|w| {
            let h = w.resolution.height();
            if h > 0.0 {
                w.resolution.width() / h
            } else {
                1.0
            }
        })
        .unwrap_or(1.0)
        .max(0.1);
    let h_fov = 2.0 * ((0.5 * v_fov).tan() * aspect).atan();
    let half_angle = 0.5 * v_fov.min(h_fov);
    let mut distance = radius / half_angle.sin() * FRAMING_MARGIN;
    // Keep the model outside the near plane for tiny content.
    distance = distance.max(near * 1.5 + radius);

    let pos = center + VIEW_DIR.normalize() * distance;
    *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

    println!(
        "[static_mesh_fbx] content bounds: {count} mesh AABB(s) \
         min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
        min.x, min.y, min.z, max.x, max.y, max.z
    );
    println!(
        "[static_mesh_fbx] spin-invariant sphere center=({:.4}, {:.4}, {:.4}) \
         radius={:.4} camera distance={distance:.4}",
        center.x, center.y, center.z, radius
    );
}

/// One-shot report: labels loaded → show the `Mesh0` / `Mesh0/Primitive0`
/// resolution and prove the primitive handle is what `Scene0` renders.
#[allow(clippy::too_many_arguments)]
fn report_labels(
    mut report: ResMut<LabelReport>,
    framing: Res<CameraFraming>,
    asset_server: Res<AssetServer>,
    fbx_assets: Res<Assets<Fbx>>,
    fbx_meshes: Res<Assets<FbxMesh>>,
    meshes: Res<Assets<Mesh>>,
    root: Query<Entity, With<WorldAssetRoot>>,
    children: Query<&Children>,
    scene_meshes: Query<&Mesh3d>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    if report.reported || !framing.done {
        return;
    }
    let states = [
        asset_server.load_state(&report.fbx),
        asset_server.load_state(&report.mesh),
        asset_server.load_state(&report.primitive),
    ];
    if let Some(failed) = states
        .iter()
        .position(|s| matches!(s, LoadState::Failed(_)))
    {
        report.reported = true;
        eprintln!("[static_mesh_fbx] label load FAILED (index {failed}): {failed:?}");
        if let Ok(mut text) = texts.single_mut() {
            *text =
                Text::new("static_mesh_fbx — label resolution FAILED\n(see stderr for details)");
        }
        return;
    }
    if !states.iter().all(|s| matches!(s, LoadState::Loaded)) {
        return;
    }
    let Ok(root) = root.single() else {
        return;
    };

    // Every Scene0 mesh handle — `Primitive0` must be one of them.
    let scene_handles: Vec<Handle<Mesh>> = children
        .iter_descendants(root)
        .filter_map(|e| scene_meshes.get(e).ok().map(|m| m.0.clone()))
        .collect();
    if scene_handles.is_empty() {
        return;
    }
    assert!(
        scene_handles.contains(&report.primitive),
        "Scene0 does not render FbxAssetLabel::Primitive{{0,0}} — \
         label/scene mesh handles diverged"
    );

    let fbx = fbx_assets.get(&report.fbx).expect("Fbx index asset loaded");
    let fbx_mesh = fbx_meshes
        .get(&report.mesh)
        .expect("Mesh0 FbxMesh asset loaded");
    let primitive = meshes
        .get(&report.primitive)
        .expect("Mesh0/Primitive0 mesh asset loaded");
    let prim_name = fbx_mesh
        .primitives
        .first()
        .and_then(|p| p.name.as_deref())
        .unwrap_or("(unnamed)");
    let verts = primitive.count_vertices();

    println!(
        "[static_mesh_fbx] Fbx: meshes={} materials={} nodes={}",
        fbx.meshes.len(),
        fbx.materials.len(),
        fbx.nodes.len()
    );
    println!(
        "[static_mesh_fbx] Mesh0 '{}' → Primitive0 '{}': {verts} verts; \
         Scene0 renders Mesh0/Primitive0: yes ({} scene mesh handle(s))",
        fbx_mesh.name,
        prim_name,
        scene_handles.len()
    );

    let scale = report.scale;
    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "static_mesh_fbx — {}\n\
             Fbx: meshes={} materials={} nodes={} | Mesh0 '{}' -> Primitive0 '{}' ({verts} verts)\n\
             Scene0 Mesh3d handles include Mesh0/Primitive0 [OK] (label = rendered geometry)\n\
             auto-framed to measured bounds; demo scale = {scale}.",
            report.path,
            fbx.meshes.len(),
            fbx.materials.len(),
            fbx.nodes.len(),
            fbx_mesh.name,
            prim_name,
        ));
    }
    report.reported = true;
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
