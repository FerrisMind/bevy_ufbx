//! Imported FBX lights (and cameras) illuminating a mesh.
//!
//! ```sh
//! cargo run --example lights_cameras_fbx
//! ```
//!
//! Lights/cameras: `assets/maya_camera_light_axes_y_up_6100_binary.fbx`
//! (no mesh in that fixture). Lit subject: Bevy cube + ground.
//!
//! Ambient is near-zero and the app DirectionalLight is removed once the FBX
//! scene is ready so the imported DirectionalLight is the key light.
//!
//! The loader activates the first camera in scene order itself (`Camera.is_active`,
//! glTF parity — see `first_camera_in_scene_order_is_active_and_only_it` in
//! `tests/parity_contract.rs`), so this example only VERIFIES on
//! `WorldInstanceReady` that exactly one imported camera is active, then
//! despawns the bootstrap framing camera. The loader also sets light `range`
//! (FBX-authored attenuation, default 20.0) and mirrors it onto
//! `SpotLight.radius` — the observer checks that invariant, nothing here
//! writes either field.

use std::path::Path;

use bevy::{prelude::*, world_serialization::WorldInstanceReady};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const LIGHTS_CAMERAS: &str = "maya_camera_light_axes_y_up_6100_binary.fbx";

fn main() {
    App::new()
        // Near-zero ambient: FBX light must carry the scene.
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 40.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — lights_cameras_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, rotate_subject)
        .run();
}

#[derive(Component)]
struct LitSubject;

#[derive(Component)]
struct FbxLightScene;

/// Bootstrap camera used only until the loader-activated FBX camera takes over.
#[derive(Component)]
struct FramingCamera;

/// Weak fill light removed once the FBX DirectionalLight is present.
#[derive(Component)]
struct AppFillLight;

#[derive(Component)]
struct StatusText;

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assert!(
        Path::new("assets").join(LIGHTS_CAMERAS).is_file(),
        "missing assets/{LIGHTS_CAMERAS} — copy from ufbx/data"
    );

    // Imported lights + camera (no meshes in this fixture). The loader already
    // activated the first camera in scene order by the time Scene0 spawns.
    commands
        .spawn((
            FbxLightScene,
            WorldAssetRoot(asset_server.load(FbxAssetLabel::Scene(0).from_asset(LIGHTS_CAMERAS))),
            Transform::IDENTITY,
        ))
        .observe(report_fbx_scene_ready);

    // Subject mesh — not from the light fixture (that file has no geometry).
    commands.spawn((
        LitSubject,
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.75, 0.55),
            perceptual_roughness: 0.55,
            metallic: 0.05,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.22, 0.24, 0.26))),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // Framing camera until the observer confirms the loader-activated FBX camera.
    commands.spawn((
        FramingCamera,
        Camera3d::default(),
        Transform::from_xyz(3.5, 2.5, 5.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));

    // Temporary fill only — despawned when FBX light is ready (avoids black flash).
    commands.spawn((
        AppFillLight,
        Transform::from_rotation(Quat::from_euler(
            EulerRot::ZYX,
            0.0,
            0.3,
            -std::f32::consts::PI / 5.,
        )),
        DirectionalLight {
            illuminance: 200.0,
            shadow_maps_enabled: false,
            ..default()
        },
    ));

    commands.spawn((
        StatusText,
        Text::new(
            "lights_cameras_fbx — maya_camera_light_axes_y_up_6100_binary.fbx\n\
             Waiting for Scene0 — loader activates the FBX camera; observer verifies it\n\
             and drops the app fill light. Fixture has lights+camera only (probe = Cuboid).",
        ),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

#[allow(clippy::too_many_arguments)]
fn report_fbx_scene_ready(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    cameras: Query<&Camera>,
    dir_lights: Query<&DirectionalLight>,
    point_lights: Query<&PointLight>,
    spot_lights: Query<&SpotLight>,
    framing: Query<Entity, With<FramingCamera>>,
    fill: Query<Entity, With<AppFillLight>>,
    mut commands: Commands,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    let mut dir = 0usize;
    let mut pts = 0usize;
    let mut spots = 0usize;
    let mut cams = 0usize;
    let mut active = 0usize;
    let mut bad_spot_radius = 0usize;

    for child in children.iter_descendants(ready.entity) {
        if dir_lights.get(child).is_ok() {
            dir += 1;
        }
        if point_lights.get(child).is_ok() {
            pts += 1;
        }
        if let Ok(spot) = spot_lights.get(child) {
            spots += 1;
            // Loader contract: `SpotLight.radius` mirrors the light `range`
            // (FBX-authored attenuation, default 20.0 when unauthored).
            if (spot.radius - spot.range).abs() > f32::EPSILON {
                bad_spot_radius += 1;
            }
        }
        if let Ok(cam) = cameras.get(child) {
            cams += 1;
            if cam.is_active {
                active += 1;
            }
        }
    }

    // The loader activates the first camera in scene order (glTF parity) — the
    // example never writes `is_active`, it only demonstrates the invariant.
    assert!(
        cams == 0 || active == 1,
        "loader must activate exactly one imported camera (active={active} of {cams})"
    );
    assert_eq!(
        bad_spot_radius, 0,
        "loader must keep SpotLight.radius == range for every spot light"
    );

    if active == 1 {
        for entity in &framing {
            commands.entity(entity).despawn();
        }
    }

    // FBX DirectionalLight is the key light — remove the app fill.
    for entity in &fill {
        commands.entity(entity).despawn();
    }

    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "lights_cameras_fbx — {LIGHTS_CAMERAS}\n\
             FBX: {dir} Directional, {pts} Point, {spots} Spot (radius == range), \
             {cams} Camera(s); loader-activated active={active}/1.\n\
             App fill light removed — FBX light is the key illuminant.\n\
             Probe Cuboid remains (fixture has no mesh)."
        ));
    }

    info!(
        "FBX scene ready: {dir} DirectionalLight(s), {pts} PointLight(s), {spots} SpotLight(s), \
         {cams} Camera(s); loader-activated active={active}"
    );
}

fn rotate_subject(time: Res<Time>, mut q: Query<&mut Transform, With<LitSubject>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.35);
    }
}
