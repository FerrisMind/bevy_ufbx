//! Negative world scale → cull-inverted `Material{N} (inverted)`.
//!
//! ```sh
//! cargo run --example neg_scale_fbx
//! ```
//!
//! Asset: `assets/blender_340_mirrored_normals_7400_binary.fbx`
//! Contains `Suzanne_Flipped` with local scale `(-1,-1,-1)` (odd negative axes
//! on world scale). The loader (`src/scene.rs`) matches `bevy_gltf`: when a
//! node's **world** scale has an odd number of negative axes and the material
//! is not double-sided, the primitive gets the cull-inverted twin labeled
//! [`FbxAssetLabel::MaterialInverted`] (`Material{N} (inverted)`, front-face
//! culling) instead of the base `StandardMaterial` — so the mirrored mesh is
//! not inside-out and stays visible.
//!
//! The on-screen report counts: spawned `Mesh3d`s, `Mesh3d`s whose world
//! transform hits the odd-negative rule, `Front`-cull materials, and the
//! actual asset path of the inverted material. Camera auto-frames the
//! measured content bounds.
//! Look for: two Suzannes (normal + mirrored); mirrored uses inverted cull.

use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::{
    camera::primitives::Aabb,
    light::CascadeShadowConfigBuilder,
    prelude::*,
    render::render_resource::Face,
    window::PrimaryWindow,
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "blender_340_mirrored_normals_7400_binary.fbx";
/// Blender Auto → AdjustTransforms; metre-ish extents.
const DEMO_VISUAL_SCALE: f32 = 1.0;

/// Frames to poll for mesh `Aabb`s before keeping the default framing.
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Frustum margin on the fitted distance (bounding sphere × slack).
const FRAMING_MARGIN: f32 = 1.15;
/// Viewpoint direction of the original hard-coded camera — the auto-fit
/// camera sits at the same diagonal angle, just at the measured distance.
const VIEW_DIR: Vec3 = Vec3::new(4.5, 2.8, 5.5);

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — neg_scale_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .init_resource::<CameraFraming>()
        .init_resource::<ReportFlag>()
        .add_systems(Startup, setup)
        .add_systems(Update, (rotate, report_inverted_cull, frame_camera))
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

/// One-shot on-screen report flag.
#[derive(Resource, Default)]
struct ReportFlag {
    reported: bool,
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
        .observe(scene_ready);

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_translation(VIEW_DIR * 3.5).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Ground,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.28, 0.4, 0.32))),
        Transform::from_xyz(0.0, -1.2, 0.0),
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
            "neg_scale_fbx — {FBX_PATH}\n\
             Look for: mirrored Suzanne with Front-cull (Material inverted twin).\n\
             Counting Mesh3d / Front cull after spawn..."
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

/// Scene instance spawned → arm the fit-to-content camera framing.
fn scene_ready(_ready: On<WorldInstanceReady>, mut framing: ResMut<CameraFraming>) {
    if !framing.done {
        framing.armed = true;
        framing.frames = 0;
    }
}

/// Fit the camera to the measured content bounds (the `load_fbx` pattern),
/// using a **spin-invariant** bounding sphere around the Y axis through the
/// origin (the example spins the two Suzannes). The ground plane is re-fitted
/// from the same measurement.
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
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !framing.armed || framing.done {
        return;
    }
    framing.frames += 1;

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
                "[neg_scale_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
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
    distance = distance.max(near * 1.5 + radius);

    let pos = center + VIEW_DIR.normalize() * distance;
    *cam_tf = Transform::from_translation(pos).looking_at(center, Vec3::Y);

    println!(
        "[neg_scale_fbx] content bounds: {count} mesh AABB(s) \
         min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
        min.x, min.y, min.z, max.x, max.y, max.z
    );
    println!(
        "[neg_scale_fbx] spin-invariant sphere center=({:.4}, {:.4}, {:.4}) \
         radius={:.4} camera distance={distance:.4}",
        center.x, center.y, center.z, radius
    );
}

/// One-shot report once `Scene0` bounds exist: count spawned `Mesh3d`s,
/// `Mesh3d`s whose **world** transform satisfies the loader's odd-negative
/// scale rule (the exact predicate from `src/scene.rs`), `Front`-cull
/// materials, and the asset path of the inverted material actually used.
#[allow(clippy::too_many_arguments)]
fn report_inverted_cull(
    framing: Res<CameraFraming>,
    mut flag: ResMut<ReportFlag>,
    asset_server: Res<AssetServer>,
    root: Query<Entity, With<WorldAssetRoot>>,
    children: Query<&Children>,
    mesh3d: Query<&Mesh3d>,
    globals: Query<&GlobalTransform>,
    mesh_mats: Query<&MeshMaterial3d<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    if flag.reported || !framing.done {
        return;
    }
    let Ok(root) = root.single() else {
        return;
    };
    let mut mesh_count = 0usize;
    let mut odd_neg = 0usize;
    let mut front_cull = 0usize;
    let mut inverted_path: Option<String> = None;
    for child in children.iter_descendants(root) {
        if mesh3d.get(child).is_err() {
            continue;
        }
        mesh_count += 1;

        // The loader's rule (src/scene.rs): odd number of negative axes on
        // the node's **world** scale → the primitive gets the cull-inverted
        // material twin. Same predicate, measured on the spawned entities.
        if let Ok(global) = globals.get(child) {
            let world = Transform::from_matrix(Mat4::from(global.affine()));
            if world.scale.is_negative_bitmask().count_ones() & 1 == 1 {
                odd_neg += 1;
            }
        }

        if let Ok(mm) = mesh_mats.get(child)
            && let Some(mat) = materials.get(&mm.0)
            && mat.cull_mode == Some(Face::Front)
        {
            front_cull += 1;
            if inverted_path.is_none() {
                inverted_path = asset_server.get_path(mm.0.id()).map(|p| p.to_string());
            }
        }
    }

    println!(
        "[neg_scale_fbx] Scene0 Mesh3d={mesh_count}; odd-neg world-scale={odd_neg}; \
         Front cull={front_cull}"
    );
    println!(
        "[neg_scale_fbx] inverted material asset: {}",
        inverted_path.as_deref().unwrap_or("(none found)")
    );

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "neg_scale_fbx — {FBX_PATH}\n\
             Look for: normal + mirrored Suzanne; mirrored uses inverted (Front) cull.\n\
             Scene0 Mesh3d={mesh_count} | odd-neg world-scale Mesh3d={odd_neg} | \
             Front-cull materials={front_cull}\n\
             Inverted material asset: {} (FbxAssetLabel::MaterialInverted).",
            inverted_path.as_deref().unwrap_or("(none found)")
        ));
    }
    flag.reported = true;
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
