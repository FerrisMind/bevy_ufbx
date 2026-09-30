//! Material-split primitives on one mesh (multi-material Suzanne).
//!
//! ```sh
//! cargo run --example multimaterial_fbx
//! ```
//!
//! Asset: `assets/blender_suzanne_multimaterial_7400_binary.fbx`
//! Look for: one Suzanne with several color regions — the scene spawns 7
//! Mesh3d, one per primitive, each with its own material (fixture dump: 7
//! primitives = 7 materials: LeftEye, Monkey, LeftEar, RightEar, RightEye,
//! Nose, Pupil). Blender Auto → AdjustTransforms; demo scale stays 1.0
//! (metre-ish extents) and the camera fits the measured, spin-invariant
//! content bounds instead of a hard-coded position.

use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::camera::primitives::Aabb;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::world_serialization::WorldInstanceReady;
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blender_suzanne_multimaterial_7400_binary.fbx";
/// Blender AdjustTransforms → metre-ish; keep 1.0 and frame with a fitted camera.
const DEMO_VISUAL_SCALE: f32 = 1.0;

/// Frames to poll for mesh `Aabb`s before keeping the default camera framing
/// (`Aabb` is inserted one frame after the scene entities spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Direction of the original hard-coded camera offset — kept so the auto-fit
/// camera sits at the same diagonal angle, just at the right distance.
const VIEW_DIR: Vec3 = Vec3::new(3.2, 2.2, 4.2);
/// Frustum margin on the fitted distance.
const FRAMING_MARGIN: f32 = 1.15;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — multimaterial_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, frame_camera))
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

/// Fit-to-content state machine: armed by the `WorldInstanceReady` observer,
/// polled until `Aabb`s exist (or the timeout keeps the default framing).
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assert!(
        Path::new("assets").join(FBX_PATH).is_file(),
        "missing assets/{FBX_PATH} — copy from libs/ufbx/data"
    );

    commands
        .spawn((
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(FBX_PATH))),
            Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
            Spinning,
        ))
        .observe(report_mesh3d_count)
        .observe(scene_ready);

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        // Framing system repositions the camera once content bounds exist;
        // this is the fallback view for the timeout path.
        Transform::from_xyz(3.2, 2.4, 4.2).looking_at(Vec3::new(0.0, 0.2, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Ground,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.0, 0.0),
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
            "multimaterial_fbx - {FBX_PATH}\n\
             Look for: COLOR REGIONS on Suzanne (material-split primitives).\n\
             Expect at least 2 Mesh3d entities under Scene0 (dump shows 7).\n\
             Counting Mesh3d and distinct materials after spawn..."
        )),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Scene instance spawned → arm the fit-to-content camera framing.
fn scene_ready(_ready: On<WorldInstanceReady>, mut framing: ResMut<CameraFraming>) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
}

/// One-shot spawn report: count the material-split Mesh3d entities AND the
/// distinct materials under the root (fixture dump: 7 primitives = 7
/// materials), print both to stdout and update the on-screen status line.
fn report_mesh3d_count(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    mesh3d: Query<(), With<Mesh3d>>,
    materials: Query<&MeshMaterial3d<StandardMaterial>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    let mut count = 0usize;
    let mut mat_ids = std::collections::HashSet::new();
    for child in children.iter_descendants(ready.entity) {
        if mesh3d.get(child).is_ok() {
            count += 1;
        }
        if let Ok(mat) = materials.get(child) {
            mat_ids.insert(mat.0.id());
        }
    }
    let mats = mat_ids.len();
    println!(
        "[multimaterial_fbx] Scene0: Mesh3d={count} distinct_materials={mats} \
         (fixture dump: 7 primitives = 7 materials)"
    );
    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "multimaterial_fbx - {FBX_PATH}\n\
             Look for: COLOR REGIONS on Suzanne (material-split primitives).\n\
             Scene0 Mesh3d count: {count}, distinct materials: {mats} (dump: 7 = 7).\n\
             Blender Auto -> AdjustTransforms; DEMO_VISUAL_SCALE={DEMO_VISUAL_SCALE}."
        ));
    }
}

/// Fit the camera to the measured content bounds (the `load_fbx` pattern),
/// spin-proofed for the rotating Suzanne: the demo spins the model about the Y
/// axis through the origin, so the frame is a **spin-invariant** bounding
/// sphere around that axis (horizontal distance + height do not change as the
/// model turns, unlike the raw union `Aabb`). Ground and shadow cascades are
/// re-fitted from the same measurement.
#[allow(clippy::too_many_arguments)]
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

    // Union of world-space mesh bounds (ground excluded: it is dressing, not
    // content). `Aabb` is inserted by Bevy's calculate_bounds after spawn.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0usize;
    for (aabb, global) in meshes.iter() {
        let (lo, hi) = (aabb.min(), aabb.max());
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    min = min.min(world);
                    max = max.max(world);
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
                "[multimaterial_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
                 keeping default camera framing"
            );
        }
        return;
    }
    framing.armed = false;
    framing.done = true;

    // Spin-invariant sphere about the Y axis through the origin (the `Spinning`
    // root rotates the model about that axis).
    let center = Vec3::new(0.0, (min.y + max.y) * 0.5, 0.0);
    let mut radius = (max.y - min.y).abs() * 0.5;
    for (aabb, global) in meshes.iter() {
        let (lo, hi) = (aabb.min(), aabb.max());
        for x in [lo.x, hi.x] {
            for y in [lo.y, hi.y] {
                for z in [lo.z, hi.z] {
                    let world = global.transform_point(Vec3::new(x, y, z));
                    radius = radius.max(Vec3::new(world.x, world.y, world.z).distance(center));
                }
            }
        }
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
                w.resolution.width() as f32 / h as f32
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

    println!("[multimaterial_fbx] content bounds: {count} mesh AABB(s)");
    println!(
        "[multimaterial_fbx]   world min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4}) \
         center=({:.4}, {:.4}, {:.4}) radius={:.4} (spin-invariant)",
        min.x, min.y, min.z, max.x, max.y, max.z, center.x, center.y, center.z, radius
    );
    println!(
        "[multimaterial_fbx] camera framing: distance={distance:.4} v_fov={v_fov:.4}rad \
         aspect={aspect:.3} translation=({:.4}, {:.4}, {:.4})",
        pos.x, pos.y, pos.z
    );
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
