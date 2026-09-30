//! NURBS surface tessellated to a triangle mesh.
//!
//! ```sh
//! cargo run --example nurbs_fbx
//! ```
//!
//! Asset: `assets/nurbs_saddle.fbx` (Maya cm → cm-sized geometry after
//! `ModifyGeometry`, ×100 demo scale for visibility).
//!
//! The loader does **not** keep editable NURBS: `mesh.rs` calls
//! `ufbx::NurbsSurface::tessellate` (16×16 span subdivision) and exports the
//! result as an ordinary `Mesh0` / `Mesh0/Primitive0` pair — this example
//! loads those labels and shows the tessellated vertex count on screen. The
//! tessellated node has no ufbx `node.mesh`, so the loader inserts no explicit
//! `Aabb`; Bevy computes the bounds at runtime (the auto-framing below reads
//! them). Look for: smooth saddle surface spinning (triangles, not raw NURBS).

use std::f32::consts::{FRAC_PI_4, PI};
use std::path::Path;

use bevy::{
    asset::LoadState,
    camera::primitives::Aabb,
    light::CascadeShadowConfigBuilder,
    prelude::*,
    window::PrimaryWindow,
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use bevy_ufbx::{FbxAssetLabel, FbxMesh, FbxPlugin};

const FBX_PATH: &str = "nurbs_saddle.fbx";
const DEMO_VISUAL_SCALE: f32 = 100.0;

/// Frames to poll for mesh `Aabb`s before keeping the default framing
/// (a tessellated NURBS node gets its `Aabb` from Bevy's runtime
/// calculate_bounds, one frame after spawn).
const FRAMING_TIMEOUT_FRAMES: u32 = 600;
/// Frustum margin on the fitted distance (bounding sphere × slack).
const FRAMING_MARGIN: f32 = 1.15;
/// Viewpoint direction of the original hard-coded camera — the auto-fit
/// camera sits at the same diagonal angle, just at the measured distance.
const VIEW_DIR: Vec3 = Vec3::new(2.5, 2.2, 4.0);

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 2000.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — nurbs_fbx".into(),
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
/// `WorldInstanceReady` observer, polled until runtime `Aabb`s exist.
#[derive(Resource, Default)]
struct CameraFraming {
    armed: bool,
    frames: u32,
    done: bool,
}

/// The mesh labels this example proves, plus the one-shot report flag.
#[derive(Resource)]
struct LabelReport {
    mesh: Handle<FbxMesh>,
    primitive: Handle<Mesh>,
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

    // The tessellated NURBS is exported as a regular labeled mesh pair —
    // `report_labels` verifies that on screen.
    commands.insert_resource(LabelReport {
        mesh: asset_server.load(FbxAssetLabel::Mesh(0).from_asset(FBX_PATH)),
        primitive: asset_server.load(
            FbxAssetLabel::Primitive {
                mesh: 0,
                primitive: 0,
            }
            .from_asset(FBX_PATH),
        ),
        reported: false,
    });

    commands.spawn((
        Camera3d::default(),
        ViewerCamera,
        // near=0.01 (Bevy default 0.1) lets fit-to-content framing get close
        // to small content.
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_translation(VIEW_DIR * 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Ground,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.45, 0.35))),
        Transform::from_xyz(0.0, -0.6, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 15.0,
            maximum_distance: 30.0,
            ..default()
        }
        .build(),
    ));

    commands.spawn((
        StatusText,
        Text::new(
            "nurbs_fbx — nurbs_saddle.fbx\n\
             Look for: tessellated saddle surface (triangle mesh), spinning.\n\
             Native NURBS is not rendered; loader tessellates via ufbx.\n\
             Waiting for Scene0 + Mesh0/Primitive0 labels...",
        ),
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
/// origin (the example spins the model, so the raw union `Aabb` drifts). The
/// ground plane is re-fitted from the same measurement.
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
                "[nurbs_fbx] no mesh bounds after {FRAMING_TIMEOUT_FRAMES} frames — \
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
            ground.scale = Vec3::splat((radius * 0.7).max(1.0));
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
        "[nurbs_fbx] content bounds: {count} mesh AABB(s) \
         min=({:.4}, {:.4}, {:.4}) max=({:.4}, {:.4}, {:.4})",
        min.x, min.y, min.z, max.x, max.y, max.z
    );
    println!(
        "[nurbs_fbx] spin-invariant sphere center=({:.4}, {:.4}, {:.4}) \
         radius={:.4} camera distance={distance:.4}",
        center.x, center.y, center.z, radius
    );
}

/// One-shot report: labels loaded → show that the tessellated NURBS became a
/// normal `Mesh0` / `Mesh0/Primitive0` pair and that `Scene0` renders it.
#[allow(clippy::too_many_arguments)]
fn report_labels(
    mut report: ResMut<LabelReport>,
    framing: Res<CameraFraming>,
    asset_server: Res<AssetServer>,
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
    let mesh_state = asset_server.load_state(&report.mesh);
    let prim_state = asset_server.load_state(&report.primitive);
    if matches!(mesh_state, LoadState::Failed(_)) || matches!(prim_state, LoadState::Failed(_)) {
        report.reported = true;
        eprintln!("[nurbs_fbx] Mesh0 / Primitive0 label load FAILED");
        if let Ok(mut text) = texts.single_mut() {
            *text = Text::new("nurbs_fbx — label resolution FAILED\n(see stderr)");
        }
        return;
    }
    if !matches!(mesh_state, LoadState::Loaded) || !matches!(prim_state, LoadState::Loaded) {
        return;
    }
    let Ok(root) = root.single() else {
        return;
    };
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
         tessellated NURBS label/scene handles diverged"
    );

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
        "[nurbs_fbx] Mesh0 '{}' → Primitive0 '{}': {verts} verts; \
         Scene0 renders it: yes ({} scene mesh handle(s))",
        fbx_mesh.name,
        prim_name,
        scene_handles.len()
    );

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "nurbs_fbx — {FBX_PATH}\n\
             Look for: smooth SADDLE — tessellated triangles, NOT editable NURBS.\n\
             Mesh0 '{}' -> Primitive0 '{}' ({verts} verts, ufbx 16x16 span tessellation)\n\
             Scene0 Mesh3d handles include Mesh0/Primitive0 [OK] | demo scale = {DEMO_VISUAL_SCALE}.",
            fbx_mesh.name, prim_name,
        ));
    }
    report.reported = true;
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.45);
    }
}
