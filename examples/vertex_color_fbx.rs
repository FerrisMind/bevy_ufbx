//! Vertex colors from FBX (`Mesh::ATTRIBUTE_COLOR` × white `StandardMaterial`).
//!
//! ```sh
//! cargo run --example vertex_color_fbx
//! ```
//!
//! Asset: `assets/zbrush_vertex_color_7500_ascii.fbx`
//! Look for: per-vertex painted colors on the mesh (not a flat material tint).
//! Bevy PBR multiplies vertex colors by `base_color`; this demo forces white
//! base + high roughness after spawn so the ATTRIBUTE_COLOR reads clearly.

use std::f32::consts::PI;
use std::path::Path;

use bevy::{
    light::CascadeShadowConfigBuilder, prelude::*, world_serialization::WorldInstanceReady,
};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const FBX_PATH: &str = "zbrush_vertex_color_7500_ascii.fbx";
/// Fixture is tiny — `dump_fbx` reports `Aabb min=(-0.01, 0, -0.01)`
/// `max=(0.01, 0.02, 0.01)` (0.02 units at `unit_scale=1`, NOT metre-scale as
/// this comment used to claim). Kept at 1.0 = the loader's authored size; the
/// camera below is hard-coded to the fit-to-content framing of those bounds
/// instead ("camera too far from the model" was the old 4.7-unit distance).
const DEMO_VISUAL_SCALE: f32 = 1.0;

fn main() {
    App::new()
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 1200.,
            ..default()
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_ufbx — vertex_color_fbx".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FbxPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, rotate)
        .run();
}

#[derive(Component)]
struct Spinning;

#[derive(Component)]
struct StatusText;

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
        .observe(emphasize_vertex_colors);

    // Fit-to-content framing for the 0.02-unit fixture: bounds from `dump_fbx`
    // (center (0, 0.01, 0), radius 0.0173) + the original diagonal viewpoint
    // direction at distance 0.054 → ~45% screen fill. `near: 0.01` (Bevy
    // default 0.1) is required — at the default near plane the model sits
    // inside it and stays invisible; Bevy's reverse-infinite-Z projection
    // keeps depth precision fine at this near/far ratio.
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 0.01,
            ..default()
        }),
        Transform::from_xyz(0.032, 0.024, 0.041).looking_at(Vec3::new(0.0, 0.01, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.22, 0.24))),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 0.8, -PI / 4.)),
        DirectionalLight {
            illuminance: 8_000.0,
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
        // ASCII only on screen: Bevy's default font (FiraMono-subset) has no
        // glyphs for "—", "…" and "×" (they render as tofu boxes).
        Text::new(format!(
            "vertex_color_fbx - {FBX_PATH}\n\
             Look for: painted VERTEX COLORS (ATTRIBUTE_COLOR x white base).\n\
             Waiting for Scene0..."
        )),
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

fn emphasize_vertex_colors(
    ready: On<WorldInstanceReady>,
    children: Query<&Children>,
    mesh3d: Query<&Mesh3d>,
    mesh_mats: Query<&MeshMaterial3d<StandardMaterial>>,
    meshes: Res<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut texts: Query<&mut Text, With<StatusText>>,
) {
    let mut colored = 0usize;
    for child in children.iter_descendants(ready.entity) {
        let Ok(m3d) = mesh3d.get(child) else {
            continue;
        };
        let Some(mesh) = meshes.get(&m3d.0) else {
            continue;
        };
        if mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none() {
            continue;
        }
        colored += 1;
        if let Ok(mm) = mesh_mats.get(child)
            && let Some(mut mat) = materials.get_mut(&mm.0)
        {
            // Vertex colors are multiplied by base_color in Bevy PBR.
            mat.base_color = Color::WHITE;
            mat.perceptual_roughness = 0.9;
            mat.metallic = 0.0;
        }
    }
    if let Ok(mut text) = texts.single_mut() {
        *text = Text::new(format!(
            "vertex_color_fbx - {FBX_PATH}\n\
             Look for: painted VERTEX COLORS (not a flat Lambert tint).\n\
             Mesh3d with ATTRIBUTE_COLOR: {colored}; base forced WHITE for multiply.\n\
             DEMO_VISUAL_SCALE={DEMO_VISUAL_SCALE}."
        ));
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
