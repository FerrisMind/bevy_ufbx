//! Sampler / settings precedence, four instances of one FBX side by side.
//!
//! ```sh
//! cargo run --example sampler_settings_fbx
//! ```
//!
//! Demonstrates the full precedence chain from `DefaultFbxImageSampler`'s docs
//! (strongest first, each tier wins over everything below it):
//!
//! 1. `FbxLoaderSettings::override_sampler` — used as-is for every texture;
//!    FBX wrap modes are ignored.
//! 2. `FbxLoaderSettings::default_sampler` — per-load base; FBX `wrap_u`/`wrap_v`
//!    overwrite the address modes on top of it. The loader detects "explicitly
//!    set" by `!= ImageSamplerDescriptor::default()` (the serde default is the
//!    "unset" sentinel), so even a `label` makes it count.
//! 3. `DefaultFbxImageSampler` app resource — base for loads that left the
//!    per-load field at its default value.
//! 4. Loader built-in default — `ImageSamplerDescriptor::linear()` (glTF
//!    parity, `GltfPlugin::default()` uses it too) when the resource itself
//!    is untouched.
//!
//! ## Why four asset sources (the honest part)
//!
//! bevy_asset keys a load by the full `AssetPath` — `(source, path, label)` —
//! and returns the *first* load's result for every later request of the same
//! key: settings are applied per loader run, so "spawn the same path twice
//! with different settings" silently keeps only the first settings. To get
//! four independent loader runs over the same bytes, this example loads the
//! fixture through four named asset sources (`s_builtin`, `s_resource`,
//! `s_perload`, `s_override`). Distinct sources = distinct keys = four runs.
//! The `DefaultFbxImageSampler` resource is global, so the run order is
//! phase-gated: `s_builtin` loads first while the resource is untouched
//! (tier 4); only after it settles is the resource set to Nearest, then
//! `s_resource` / `s_perload` / `s_override` load (tiers 3 / 2 / 1).
//!
//! The fixture references `textures/checkerboard_diffuse.png` relative to the
//! asset root, which is not vendored into `assets/`. External texture bytes
//! are always read through the **default** source with plain paths, so the
//! default source's reader is swapped for a router: `*.fbx` → `assets/`,
//! anything else → the ufbx corpus at `libs/ufbx/data` (optional — without it
//! the example still runs and says so; textures just stay untextured).
//! No binary fixtures are added.
//!
//! Look for: four cubes side by side, one per tier, each with a name plate
//! under its column (plate X = the column's projected screen position) and a
//! **texture swatch quad above it** showing that tier's resolved sampler on
//! the loaded checkerboard (UVs span 0..3, so Repeat tiles 3×3 while
//! ClampToEdge stretches one tile; Nearest is blocky, Linear smooth). Stdout
//! prints the requested tier per instance, the fit-to-content bounds + camera
//! + NDC frustum check, and the resolved `ImageSamplerDescriptor` per source
//! after all loads settle (the ground truth for the precedence chain).
//!
//! Honest limitation of the only fixture in `assets/` that references external
//! textures: `blender_279_internal_textures_7400_binary.fbx`'s mesh `Cube.001`
//! has **zero UV sets** (Blender "Generated" coordinates are not exported to
//! FBX), so the checkerboard cannot map onto the cubes — they render as flat
//! gray, and no fixture in `assets/` offers external textures *and* UVs. That
//! is what the swatch quads are for: each binds the tier's own
//! `FbxAssetLabel::Texture(0)` image, so the sampler differences are visible
//! on screen, not just on stdout.

use std::collections::BTreeSet;
use std::f32::consts::PI;
use std::path::Path;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
    file::FileAssetReader,
};
use bevy::asset::{AssetApp, LoadState};
use bevy::camera::primitives::Aabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::world_serialization::WorldAsset;
use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{DefaultFbxImageSampler, FbxAssetLabel, FbxLoaderSettings, FbxPlugin};

const FIXTURE: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Blender 2.79 fixture loads **metre-ish** after `AdjustTransforms` unit
/// conversion (a ~2 m cube — measured from its mesh `Aabb`s below). A naive
/// cm-style `×100` blows each instance up to ~240 units, puts the camera
/// *inside* the geometry, and backface culling hides everything (the bug this
/// example used to have). Scale 1.0 = authored size; the framing pass then
/// positions the camera from the real bounds.
const DEMO_VISUAL_SCALE: f32 = 1.0;

/// Named sources: one per precedence tier, all routing to the same files.
const SRC_BUILTIN: &str = "s_builtin";
const SRC_RESOURCE: &str = "s_resource";
const SRC_PERLOAD: &str = "s_perload";
const SRC_OVERRIDE: &str = "s_override";

/// ufbx corpus root (fixture's external PNGs). Optional at runtime.
const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../libs/ufbx/data");

/// World-space columns, left → right: tiers 4, 3, 2, 1.
const COLUMN_X: [f32; 4] = [-4.5, -1.5, 1.5, 4.5];

/// Texture swatch quads sit above the cubes — the fixture cube has no UVs, so
/// the resolved samplers would be invisible without them. The boxes are folded
/// into the framing union explicitly (`frame_scene`): the swatches are not
/// children of the spinning instance roots.
const SWATCH_Y: f32 = 2.35;
const SWATCH_SIZE: f32 = 1.7;

fn main() {
    let mut app = App::new();

    // Distinct asset sources = distinct AssetPath keys = independent loader
    // runs over the same FBX bytes (see header). Must run BEFORE DefaultPlugins
    // builds the AssetServer.
    for id in [SRC_BUILTIN, SRC_RESOURCE, SRC_PERLOAD, SRC_OVERRIDE] {
        app.register_asset_source(
            id,
            AssetSourceBuilder::new(|| Box::new(RoutingReader::new())),
        );
    }
    // External texture reads/fallbacks always use the DEFAULT source with plain
    // paths, so it must route too: `*.fbx` → assets/, everything else → corpus.
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(|| Box::new(RoutingReader::new())),
    );

    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        // Toned down from the 2000/12000 pair other examples copy: at this
        // close framing those levels clip every cube face to pure white and
        // the face-to-face shading gradient disappears.
        brightness: 700.,
        ..default()
    })
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bevy_ufbx — sampler_settings_fbx".into(),
            // Fixed size → deterministic plate layout (plates are placed at
            // projected pixel X positions of the four columns).
            resolution: (1600u32, 900u32).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FbxPlugin)
    .add_systems(Startup, setup)
    .add_systems(Update, (rotate, advance))
    .run();
}

// ----------------------------------------------------------------------------
// Routing asset source
// ----------------------------------------------------------------------------

/// Serves `*.fbx` from `assets/` and everything else from the ufbx corpus.
///
/// Replaces the reader of every source used here (named ones and the default),
/// because the loader's texture pre-reads and fallback loads always go through
/// the default source with plain paths.
struct RoutingReader {
    assets: FileAssetReader,
    corpus: FileAssetReader,
}

impl RoutingReader {
    fn new() -> Self {
        Self {
            assets: FileAssetReader::new("assets"),
            // Absolute path replaces the base path inside FileAssetReader::new.
            corpus: FileAssetReader::new(CORPUS),
        }
    }
}

fn is_fbx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("fbx"))
}

impl AssetReader for RoutingReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if is_fbx(path) {
            self.assets.read(path).await
        } else {
            self.corpus.read(path).await
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if is_fbx(path) {
            self.assets.read_meta(path).await
        } else {
            self.corpus.read_meta(path).await
        }
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.assets.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        self.assets.is_directory(path).await
    }
}

// ----------------------------------------------------------------------------
// Precedence demo
// ----------------------------------------------------------------------------

#[derive(Component)]
struct Spinning;

/// The backdrop plane — re-seated under the framed content in `Phase::Framing`.
#[derive(Component)]
struct Floor;

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    /// Tier 4: load s_builtin with the resource untouched.
    Builtin,
    /// Tiers 3/2/1: resource set to Nearest, then s_resource/s_perload/s_override.
    Rest,
    /// All settled + scene content spawned: measure bounds, fit camera,
    /// place name plates under their columns (u32 = frames waited for content).
    Framing(u32),
    /// Framing done: print resolved samplers per source.
    Report,
    Done,
}

#[derive(Resource)]
struct Demo {
    phase: Phase,
    builtin: Handle<WorldAsset>,
    rest: Vec<Handle<WorldAsset>>,
    /// 3D camera — re-aimed by the fit-to-content pass.
    camera: Entity,
    /// Backdrop plane — re-seated under the framed union.
    floor: Entity,
    /// Instance roots in COLUMN_X order (tiers 4, 3, 2, 1).
    instances: Vec<Entity>,
}

/// Tier 2 descriptor: differs from `ImageSamplerDescriptor::default()` (label +
/// mag filter), which is how the loader detects an explicitly-set per-load
/// value. Mag Linear / min Nearest makes the column visibly between tiers 4/3.
fn per_load_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("per-load: FbxLoaderSettings::default_sampler".to_string()),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Nearest,
        ..ImageSamplerDescriptor::default()
    }
}

/// Tier 1 descriptor: used as-is — Linear filters, ClampToEdge address modes,
/// so the FBX Repeat wrap does NOT apply (the visual tell for the top tier).
fn override_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("override: FbxLoaderSettings::override_sampler".to_string()),
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    }
}

/// Tier 3 descriptor stored in the `DefaultFbxImageSampler` resource: an
/// explicit Nearest build (the built-in default is now `linear()`), so the
/// blocky column stays visibly distinct from tier 4.
fn resource_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        label: Some("app-resource: DefaultFbxImageSampler".to_string()),
        ..ImageSamplerDescriptor::nearest()
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    sampler: Res<DefaultFbxImageSampler>,
) {
    assert!(
        Path::new("assets").join(FIXTURE).is_file(),
        "missing assets/{FIXTURE} — copy from ufbx/data"
    );

    println!("sampler_settings_fbx — precedence chain, four instances of one FBX");
    println!("fixture: assets/{FIXTURE}");
    let corpus_ok = Path::new(CORPUS)
        .join("textures/checkerboard_diffuse.png")
        .is_file();
    if corpus_ok {
        println!("texture bytes: default asset source routed to the ufbx corpus ({CORPUS})");
    } else {
        println!(
            "texture bytes: ufbx corpus NOT found at {CORPUS} — external textures cannot \
             be served; cubes render untextured and the sampler report only covers images \
             that exist"
        );
    }
    print_fixture_wrap();

    println!("caching: bevy_asset keys loads by (source, path, label) and the FIRST load's");
    println!("         settings win for every later request of that key, so four named");
    println!("         asset sources give four independent loader runs over the same file:");
    println!(
        "  [4] {SRC_BUILTIN}  no settings, resource untouched   → built-in default (Linear, glTF parity)"
    );
    println!("  [3] {SRC_RESOURCE} no settings, resource = Nearest  → app resource tier");
    println!("  [2] {SRC_PERLOAD}  default_sampler ≠ default()     → per-load tier");
    println!("  [1] {SRC_OVERRIDE} override_sampler as-is          → override tier (ignores wrap)");
    println!(
        "phase 1: requesting [4] {SRC_BUILTIN} while the resource is untouched = {:?}",
        sampler.get()
    );

    // Tier 4 first, resource untouched.
    let builtin = asset_server.load::<WorldAsset>(
        FbxAssetLabel::Scene(0).from_asset(format!("{SRC_BUILTIN}://{FIXTURE}")),
    );
    let instance0 = spawn_instance(&mut commands, builtin.clone(), COLUMN_X[0]);

    // Provisional pose — `Phase::Framing` re-aims it once the real bounds are
    // known (see `fit_camera_to_bounds`).
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 2.8, 10.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        ))
        .id();

    // Overlay camera for Text2d plates (does not clear the 3D color buffer).
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));

    let floor = commands
        .spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(30.0, 14.0))),
            MeshMaterial3d(materials.add(Color::srgb(0.25, 0.28, 0.32))),
            Transform::from_xyz(0.0, -0.55, 0.0),
            Floor,
        ))
        .id();

    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        DirectionalLight {
            illuminance: 6_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 30.0,
            maximum_distance: 60.0,
            ..default()
        }
        .build(),
    ));

    // Name plates are spawned by the framing pass, once the camera is fitted:
    // their X is the projected pixel position of each column (no overlap — see
    // `spawn_name_plates`). The legend here stays authoritative for order.

    commands.spawn((
        Text::new(
            // Pure ASCII: Bevy's default font is FiraMono-*subset* — em dashes,
            // middle dots and arrows render as tofu boxes on screen.
            "sampler_settings_fbx - same FBX, four sampler-precedence tiers\n\
             LEFT->RIGHT: [4] built-in | [3] resource | [2] per-load | [1] override\n\
             bevy_asset caches by (source, path, label): same path would keep only the\n\
             first settings, so each tier loads through its own named asset source.\n\
             BOTTOM: one name plate per column, placed at the column's projected\n\
             screen position; stdout prints bounds, camera and NDC frustum check.\n\
             QUADS ABOVE THE CUBES: texture swatches (UV 0..3) bound to each\n\
             tier's own loaded image - Repeat tiles 3x3, ClampToEdge stretches;\n\
             Nearest blocky vs Linear smooth. The fixture cube has no UV sets,\n\
             so the cubes themselves render flat gray (see stdout).",
        ),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));

    commands.insert_resource(Demo {
        phase: Phase::Builtin,
        builtin,
        rest: Vec::new(),
        camera,
        floor,
        instances: vec![instance0],
    });
}

fn spawn_instance(commands: &mut Commands, handle: Handle<WorldAsset>, x: f32) -> Entity {
    commands
        .spawn((
            WorldAssetRoot(handle),
            Transform::from_xyz(x, 0.0, 0.0).with_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
            Spinning,
        ))
        .id()
}

/// Quad mesh for a texture swatch: UVs span 0..3 (scaled up from the
/// `Rectangle` 0..1 default) so the wrap-mode tiers are visible — Repeat
/// tiles 3×3 where ClampToEdge stretches one tile across the quad.
fn swatch_mesh() -> Mesh {
    let mut mesh = Mesh::from(Rectangle::new(SWATCH_SIZE, SWATCH_SIZE));
    if let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
    {
        for uv in uvs.iter_mut() {
            uv[0] *= 3.0;
            uv[1] *= 3.0;
        }
    }
    mesh
}

/// Base-color texture handle of an instance's spawned scene, found by walking
/// its mesh descendants — i.e. the `Image` that tier's loader run actually
/// produced (same asset the report section prints).
fn instance_texture(
    root: Entity,
    children: &Query<&Children>,
    mesh_materials: &Query<&MeshMaterial3d<StandardMaterial>>,
    materials: &Assets<StandardMaterial>,
) -> Option<Handle<Image>> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok(mm) = mesh_materials.get(entity) {
            if let Some(m) = materials.get(mm.0.id()) {
                if m.base_color_texture.is_some() {
                    return m.base_color_texture.clone();
                }
            }
        }
        if let Ok(ch) = children.get(entity) {
            stack.extend(ch.iter());
        }
    }
    None
}

/// One unlit swatch quad per column, bound to that tier's own texture image,
/// seated above the column's cube (the cubes carry no UVs — see header).
#[allow(clippy::too_many_arguments)]
fn spawn_swatches(
    demo: &Demo,
    asset_server: &AssetServer,
    children: &Query<&Children>,
    mesh_materials: &Query<&MeshMaterial3d<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    commands: &mut Commands,
) {
    const TIERS: [(u8, &str); 4] = [
        (4, SRC_BUILTIN),
        (3, SRC_RESOURCE),
        (2, SRC_PERLOAD),
        (1, SRC_OVERRIDE),
    ];
    let quad = meshes.add(swatch_mesh());
    println!("--- texture swatches (UV 0..3 quads above the cubes) ---");
    for (i, root) in demo.instances.iter().enumerate() {
        let (rank, source) = TIERS[i];
        let Some(texture) = instance_texture(*root, children, mesh_materials, materials) else {
            println!("  [{rank}] {source}: no base_color_texture — swatch skipped");
            continue;
        };
        let path = asset_server
            .get_path(texture.id())
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<image without path>".to_string());
        commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(texture),
                // Unlit: the swatch shows the raw sampler output, unaffected
                // by scene lighting.
                unlit: true,
                ..default()
            })),
            Transform::from_xyz(COLUMN_X[i], SWATCH_Y, 0.0),
        ));
        println!(
            "  [{rank}] {source}: swatch at ({:.2}, {:.2}, 0) bound to {path}",
            COLUMN_X[i], SWATCH_Y
        );
    }
    println!("----------------------------------------------------");
}

#[allow(clippy::too_many_arguments)]
fn advance(
    mut demo: ResMut<Demo>,
    asset_server: Res<AssetServer>,
    sampler: Res<DefaultFbxImageSampler>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    children: Query<&Children>,
    mesh_materials: Query<&MeshMaterial3d<StandardMaterial>>,
    mesh3ds: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mesh_bounds: Query<(&Aabb, &GlobalTransform)>,
    root_transforms: Query<&GlobalTransform>,
    projections: Query<&Projection, With<Camera3d>>,
    mut transforms: Query<&mut Transform>,
    windows: Query<&Window>,
) {
    match demo.phase {
        Phase::Builtin => {
            if !settled(&asset_server, &demo.builtin) {
                return;
            }
            println!(
                "[4] {SRC_BUILTIN} settled; resource at its load was untouched = {:?}",
                sampler.get()
            );
            println!("    → built-in tier: Linear filters (glTF parity), FBX wrap applied on top");

            // Tier 3/2/1 only: mutate the global resource AFTER tier 4 settled.
            sampler.set(&resource_sampler());
            println!(
                "phase 2: DefaultFbxImageSampler.set({:?}) — applies to loads requested from now on",
                resource_sampler()
            );

            let resource_handle = asset_server.load::<WorldAsset>(
                FbxAssetLabel::Scene(0).from_asset(format!("{SRC_RESOURCE}://{FIXTURE}")),
            );
            println!(
                "[3] {SRC_RESOURCE}: no per-load settings → resolve_default_sampler picks the \
                 resource ({:?}) over the built-in default",
                resource_sampler()
            );
            demo.instances.push(spawn_instance(
                &mut commands,
                resource_handle.clone(),
                COLUMN_X[1],
            ));

            let per_load_handle = asset_server
                .load_builder()
                .with_settings(|s: &mut FbxLoaderSettings| {
                    s.default_sampler = per_load_sampler();
                })
                .load::<WorldAsset>(
                    FbxAssetLabel::Scene(0).from_asset(format!("{SRC_PERLOAD}://{FIXTURE}")),
                );
            println!(
                "[2] {SRC_PERLOAD}: default_sampler = {:?} — != ImageSamplerDescriptor::default() \
                 so the loader treats it as set and it beats the Nearest resource",
                per_load_sampler()
            );
            demo.instances.push(spawn_instance(
                &mut commands,
                per_load_handle.clone(),
                COLUMN_X[2],
            ));

            let override_handle = asset_server
                .load_builder()
                .with_settings(|s: &mut FbxLoaderSettings| {
                    s.override_sampler = Some(override_sampler());
                })
                .load::<WorldAsset>(
                    FbxAssetLabel::Scene(0).from_asset(format!("{SRC_OVERRIDE}://{FIXTURE}")),
                );
            println!(
                "[1] {SRC_OVERRIDE}: override_sampler = {:?} — used as-is (beats per-load, \
                 resource, built-in, and the FBX Repeat wrap)",
                override_sampler()
            );
            demo.instances.push(spawn_instance(
                &mut commands,
                override_handle.clone(),
                COLUMN_X[3],
            ));

            demo.rest = vec![resource_handle, per_load_handle, override_handle];
            demo.phase = Phase::Rest;
        }
        Phase::Rest => {
            if demo
                .rest
                .iter()
                .all(|handle| settled(&asset_server, handle))
            {
                demo.phase = Phase::Framing(0);
            }
        }
        Phase::Framing(ticks) => {
            // Scene contents (WorldAsset → children + mesh `Aabb`s) land a few
            // frames after LoadState::Loaded; wait for them before measuring.
            let measured: Vec<Option<(Vec3, Vec3)>> = demo
                .instances
                .iter()
                .map(|root| instance_world_aabb(*root, &children, &mesh_bounds))
                .collect();
            if measured.iter().any(Option::is_none) && ticks < 300 {
                if ticks % 120 == 0 {
                    println!("waiting for instance scene content ({ticks}/300 frames)...");
                }
                demo.phase = Phase::Framing(ticks + 1);
                return;
            }
            // Swatches are spawned just before framing so their boxes can be
            // folded into the fitted view (they sit above the cubes).
            spawn_swatches(
                &demo,
                &asset_server,
                &children,
                &mesh_materials,
                &mut materials,
                &mut meshes,
                &mut commands,
            );
            frame_scene(
                &demo,
                &measured,
                &root_transforms,
                &projections,
                &mut transforms,
                &windows,
                &mut commands,
            );
            demo.phase = Phase::Report;
        }
        Phase::Report => {
            report(
                &asset_server,
                &images,
                &materials,
                &mesh_materials,
                &mesh3ds,
                &meshes,
            );
            demo.phase = Phase::Done;
        }
        Phase::Done => {}
    }
}

/// World-space AABB of an instance's mesh descendants: each descendant's mesh
/// `Aabb` transformed through its `GlobalTransform`. `None` while the scene
/// has spawned no content yet (frames pending after `LoadState::Loaded`, or a
/// failed load).
fn instance_world_aabb(
    root: Entity,
    children: &Query<&Children>,
    mesh_bounds: &Query<(&Aabb, &GlobalTransform)>,
) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut found = false;
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok((aabb, gt)) = mesh_bounds.get(entity) {
            found = true;
            let m = gt.to_matrix();
            let c = aabb.center;
            let h = aabb.half_extents;
            for sx in [-1.0f32, 1.0] {
                for sy in [-1.0f32, 1.0] {
                    for sz in [-1.0f32, 1.0] {
                        let local = Vec3::new(c.x + sx * h.x, c.y + sy * h.y, c.z + sz * h.z);
                        let w = m.transform_point3(local);
                        min = min.min(w);
                        max = max.max(w);
                    }
                }
            }
        }
        if let Ok(ch) = children.get(entity) {
            stack.extend(ch.iter());
        }
    }
    found.then_some((min, max))
}

/// Projects `p` to NDC for a camera at `pos` with an orthonormal basis
/// (`fwd` = view direction). `None` when the point is behind the camera.
fn project_ndc(
    p: Vec3,
    pos: Vec3,
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    tan_h: f32,
    tan_v: f32,
) -> Option<Vec2> {
    let v = p - pos;
    let z = v.dot(fwd);
    (z > 0.0).then(|| Vec2::new(v.dot(right) / (z * tan_h), v.dot(up) / (z * tan_v)))
}

/// Fit-to-content pass: measure the four instances, aim the camera so the
/// spin-inflated union sits inside NDC ±0.85, seat the floor under it, and
/// spawn the four name plates at their columns' projected screen positions.
/// Every decision is printed so a headless/timeout run can verify the framing
/// from stdout alone.
fn frame_scene(
    demo: &Demo,
    measured: &[Option<(Vec3, Vec3)>],
    root_transforms: &Query<&GlobalTransform>,
    projections: &Query<&Projection, With<Camera3d>>,
    transforms: &mut Query<&mut Transform>,
    windows: &Query<&Window>,
    commands: &mut Commands,
) {
    /// Tier rank + asset source per instance, COLUMN_X order (L→R).
    const TIERS: [(u8, &str); 4] = [
        (4, SRC_BUILTIN),
        (3, SRC_RESOURCE),
        (2, SRC_PERLOAD),
        (1, SRC_OVERRIDE),
    ];
    /// Bottom-of-window plates, one per column (kept short: plate pitch on a
    /// 1600 px window is ~345 px, so every line must fit under ~300 px).
    const PLATES: [&str; 4] = [
        "4 - built-in\n(default: Linear + Repeat)",
        "3 - resource\n(Nearest, app resource)",
        "2 - per-load\n(mag Linear / min Nearest)",
        "1 - override\n(Linear + ClampToEdge)",
    ];

    let (aspect, half_w, half_h) = match windows.single() {
        Ok(w) => (
            w.resolution.width() / w.resolution.height(),
            w.resolution.width() / 2.0,
            w.resolution.height() / 2.0,
        ),
        Err(_) => (16.0 / 9.0, 800.0, 450.0),
    };
    let fov = match projections.single() {
        Ok(Projection::Perspective(p)) => p.fov,
        _ => PI / 4.0,
    };
    let tan_v = (fov / 2.0).tan();
    let tan_h = tan_v * aspect;

    // Per-instance bounds. A failed load degrades to a point at the root so
    // framing (and the plates) still work — the error itself is logged by
    // `settled`.
    let per_root: Vec<(Vec3, Vec3)> = measured
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let fallback = root_transforms
                .get(demo.instances[i])
                .map(|gt| gt.translation())
                .unwrap_or(Vec3::new(COLUMN_X[i], 0.0, 0.0));
            m.unwrap_or((fallback, fallback))
        })
        .collect();

    // Union of the four instances with x/z inflated ×1.25: a spinning cube's
    // world AABB grows toward its diagonal (≈ √2 × face) as it turns, so the
    // fitted view must cover the worst rotation, not just the measured one.
    // Plus the swatch quads above each column (static, un-inflated).
    let mut union_min = Vec3::splat(f32::MAX);
    let mut union_max = Vec3::splat(f32::MIN);
    for (min, max) in &per_root {
        let center = (*min + *max) * 0.5;
        let half = (max - min) * 0.5;
        let half = Vec3::new(half.x * 1.25, half.y, half.z * 1.25);
        union_min = union_min.min(center - half);
        union_max = union_max.max(center + half);
    }
    let swatch_half = Vec3::splat(SWATCH_SIZE * 0.5);
    for x in COLUMN_X {
        let center = Vec3::new(x, SWATCH_Y, 0.0);
        union_min = union_min.min(center - swatch_half);
        union_max = union_max.max(center + swatch_half);
    }

    // Camera: same gentle downward look the example started with; distance
    // grows until every union corner projects inside NDC ±0.85.
    let target = (union_min + union_max) * 0.5;
    let view_dir = Vec3::new(0.0, 0.24, 1.0).normalize();
    let fwd = -view_dir;
    let right = fwd.cross(Vec3::Y).normalize();
    let up = right.cross(fwd);
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { union_min.x } else { union_max.x },
                if i & 2 == 0 { union_min.y } else { union_max.y },
                if i & 4 == 0 { union_min.z } else { union_max.z },
            )
        })
        .collect();
    let mut dist = 4.0f32;
    for _ in 0..64 {
        let campos = target + view_dir * dist;
        let fits = corners.iter().all(|p| {
            project_ndc(*p, campos, right, up, fwd, tan_h, tan_v)
                .is_some_and(|n| n.x.abs() <= 0.85 && n.y.abs() <= 0.85)
        });
        if fits {
            break;
        }
        dist *= 1.12;
    }
    let campos = target + view_dir * dist;
    if let Ok(mut tf) = transforms.get_mut(demo.camera) {
        *tf = Transform::from_translation(campos).looking_at(target, Vec3::Y);
    }

    // The floor is a backdrop: seat it just under the union, but keep it out
    // of the fit so its 30×14 slab cannot dominate the framing.
    if let Ok(mut tf) = transforms.get_mut(demo.floor) {
        tf.translation.y = union_min.y - 0.05;
    }

    println!("--- fit-to-content framing (4 instances) ---");
    println!(
        "  union bounds: min=({:.3},{:.3},{:.3}) max=({:.3},{:.3},{:.3}) size=({:.3},{:.3},{:.3})",
        union_min.x,
        union_min.y,
        union_min.z,
        union_max.x,
        union_max.y,
        union_max.z,
        union_max.x - union_min.x,
        union_max.y - union_min.y,
        union_max.z - union_min.z,
    );
    let mut all_inside = true;
    for (i, (min, max)) in per_root.iter().enumerate() {
        let center = (*min + *max) * 0.5;
        let half = (max - min) * 0.5;
        let half = Vec3::new(half.x * 1.25, half.y, half.z * 1.25);
        let mut ndc_min = Vec2::splat(f32::MAX);
        let mut ndc_max = Vec2::splat(f32::MIN);
        let mut inside = true;
        for k in 0..8 {
            let p = Vec3::new(
                if k & 1 == 0 {
                    center.x - half.x
                } else {
                    center.x + half.x
                },
                if k & 2 == 0 {
                    center.y - half.y
                } else {
                    center.y + half.y
                },
                if k & 4 == 0 {
                    center.z - half.z
                } else {
                    center.z + half.z
                },
            );
            match project_ndc(p, campos, right, up, fwd, tan_h, tan_v) {
                Some(n) => {
                    ndc_min = ndc_min.min(n);
                    ndc_max = ndc_max.max(n);
                    if n.x.abs() > 0.85 || n.y.abs() > 0.85 {
                        inside = false;
                    }
                }
                None => inside = false,
            }
        }
        all_inside &= inside;
        let (rank, source) = TIERS[i];
        println!(
            "  [{rank}] {source}: center=({:.3},{:.3},{:.3}) size=({:.3},{:.3},{:.3}) \
             ndc x=[{:.3},{:.3}] y=[{:.3},{:.3}] inside={}",
            center.x,
            center.y,
            center.z,
            max.x - min.x,
            max.y - min.y,
            max.z - min.z,
            ndc_min.x,
            ndc_max.x,
            ndc_min.y,
            ndc_max.y,
            if inside { "YES" } else { "NO" },
        );
    }
    println!(
        "  camera: pos=({:.3},{:.3},{:.3}) target=({:.3},{:.3},{:.3}) dist={dist:.2} \
         (fov={fov:.3} rad, aspect={aspect:.3})",
        campos.x, campos.y, campos.z, target.x, target.y, target.z,
    );
    println!(
        "  frustum check: every spin-inflated instance AABB corner inside |ndc| <= 0.85: {}",
        if all_inside { "YES" } else { "NO" }
    );

    // Name plates: X = the column center's projected screen X, Y = a bottom
    // strip (the header owns the top). The fixed 1600×900 window makes these
    // pixel positions deterministic, and one plate per column with the label
    // directly under its cube can never overlap its neighbours (~345 px pitch
    // vs ≤ 270 px label width at font 20).
    let plate_y = -half_h + 72.0;
    for (i, (min, max)) in per_root.iter().enumerate() {
        let center = (*min + *max) * 0.5;
        let ndc_x = project_ndc(center, campos, right, up, fwd, tan_h, tan_v)
            .map(|n| n.x)
            .unwrap_or(0.0);
        let px = (ndc_x * half_w).clamp(-half_w + 170.0, half_w - 170.0);
        let (rank, source) = TIERS[i];
        commands.spawn((
            Text2d::new(PLATES[i]),
            TextFont::from_font_size(18.0),
            TextColor(Color::srgb(1.0, 0.95, 0.7)),
            TextLayout::justify(Justify::Center),
            Transform::from_xyz(px, plate_y, 0.0),
        ));
        println!(
            "  plate [{rank}] {source}: screen=({px:.0},{plate_y:.0}) px — {:?}",
            PLATES[i].lines().next().unwrap_or("")
        );
    }
    println!("-------------------------------------------");
}

/// LoadState::Loaded as soon as the loader run lands its labeled assets —
/// deliberately NOT `is_loaded_with_dependencies`, which would hang the demo
/// forever when an external texture image fails to load (corpus absent).
fn settled(asset_server: &AssetServer, handle: &Handle<WorldAsset>) -> bool {
    match asset_server.load_state(handle.id()) {
        LoadState::Loaded => true,
        LoadState::Failed(err) => {
            error!("FBX load failed: {err:?}");
            true
        }
        _ => false,
    }
}

/// Ground truth: every `Image` produced by each tier's loader run, plus that
/// tier's `StandardMaterial` → base-color texture binding, plus the spawned
/// scene's mesh → material → texture wiring and UV0 sanity (why the cubes do
/// or don't show the checkerboard on screen).
fn report(
    asset_server: &AssetServer,
    images: &Assets<Image>,
    materials: &Assets<StandardMaterial>,
    mesh_materials: &Query<&MeshMaterial3d<StandardMaterial>>,
    mesh3ds: &Query<&Mesh3d>,
    meshes: &Assets<Mesh>,
) {
    println!("--- resolved image samplers per instance (ground truth: Assets<Image>) ---");
    for (rank, source) in [
        (4, SRC_BUILTIN),
        (3, SRC_RESOURCE),
        (2, SRC_PERLOAD),
        (1, SRC_OVERRIDE),
    ] {
        let mut descriptors = BTreeSet::new();
        let mut count = 0usize;
        for (id, image) in images.iter() {
            let Some(path) = asset_server.get_path(id) else {
                continue;
            };
            if path.source().as_str() != Some(source) {
                continue;
            }
            count += 1;
            descriptors.insert(match &image.sampler {
                ImageSampler::Descriptor(descriptor) => format!(
                    "{descriptor:?} ({}x{}, data {} bytes)",
                    image.texture_descriptor.size.width,
                    image.texture_descriptor.size.height,
                    image.data.as_ref().map_or(0, |d| d.len()),
                ),
                ImageSampler::Default => "ImageSampler::Default (global ImagePlugin)".to_string(),
            });
        }
        if count == 0 {
            println!("[{rank}] {source}: no images (external texture unavailable?)");
            continue;
        }
        println!("[{rank}] {source}: {count} image(s)");
        for descriptor in &descriptors {
            println!("      {descriptor}");
        }
    }
    println!(
        "chain: override_sampler > per-load default_sampler (!= default()) > \
         DefaultFbxImageSampler resource > built-in default; FBX wrap_u/v overwrite \
         address_mode_u/v unless override_sampler is set"
    );

    // Spawned-scene wiring: which material each mesh entity actually carries,
    // what it is bound to, and whether the meshes carry UV0 at all.
    println!("--- spawned scene wiring (mesh → material → texture → uv0) ---");
    let mut reported = BTreeSet::new();
    for mm in mesh_materials.iter() {
        let key = format!("{:?}", mm.0);
        if !reported.insert(key) {
            continue;
        }
        let path = asset_server
            .get_path(mm.0.id())
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<floor/inline material>".to_string());
        let info = match materials.get(mm.0.id()) {
            Some(m) => {
                let tex = m
                    .base_color_texture
                    .as_ref()
                    .and_then(|h| asset_server.get_path(h.id()))
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "None".to_string());
                format!("base_color={:?} texture={tex}", m.base_color)
            }
            None => "<material asset missing>".to_string(),
        };
        println!("  material {path}: {info}");
    }
    let (mut with_uv, mut without_uv) = (0usize, 0usize);
    for m3d in mesh3ds.iter() {
        let Some(mesh) = meshes.get(m3d.0.id()) else {
            continue;
        };
        if matches!(
            mesh.attribute(Mesh::ATTRIBUTE_UV_0),
            Some(bevy::mesh::VertexAttributeValues::Float32x2(_))
        ) {
            with_uv += 1;
        } else {
            without_uv += 1;
        }
    }
    println!(
        "  uv0: {with_uv} spawned mesh(es) with UV0 (floor plane + swatch quads), \
         {without_uv} without (the fixture cube — 'Cube.001' has 0 uv sets: Blender \
         Generated coordinates are not exported to FBX), so the checkerboard cannot map \
         onto the cubes (they render flat) — the swatch quads carry the visible sampler demo"
    );
    println!("-------------------------------------------------------------");
}

/// The fixture's authored wrap modes explain where the address modes come from.
fn print_fixture_wrap() {
    let path = format!("assets/{FIXTURE}");
    let Ok(scene) = ufbx::load_file(
        path.as_str(),
        ufbx::LoadOpts {
            load_external_files: false,
            ..Default::default()
        },
    ) else {
        return;
    };
    for texture in scene.textures.as_ref().iter() {
        if texture.content.is_empty() && !texture.filename.is_empty() {
            println!(
                "fixture texture '{}' wrap_u={:?} wrap_v={:?} (external ref → address modes \
                 come from here for tiers 4-2; tier 1 override ignores them)",
                texture.filename, texture.wrap_u, texture.wrap_v
            );
        }
    }
    // The only external-texture fixture in assets/ also happens to carry no
    // UV sets — say so up front instead of pretending the cubes will show it.
    for mesh in scene.meshes.as_ref().iter() {
        println!(
            "fixture mesh '{}' uv_sets={}{}",
            mesh.element.name,
            mesh.uv_sets.count,
            if mesh.uv_sets.count == 0 {
                " — no UVs: texture bytes load but cannot map (cubes render flat)"
            } else {
                ""
            }
        );
    }
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
