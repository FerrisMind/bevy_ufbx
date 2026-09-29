//! External FBX textures / wrap modes (the fixture references PNGs on disk).
//!
//! ```sh
//! cargo run --example textures_fbx
//! ```
//!
//! Asset: `assets/blender_279_internal_textures_7400_binary.fbx`
//! Look for: checkerboard-textured cube (base-color image), not flat grey.
//!
//! Despite the fixture's name, its textures are NOT embedded: the file
//! references `textures/checkerboard_*.png` relative to the asset root, and
//! those PNGs are not vendored into `assets/` (the ufbx corpus ships them at
//! `libs/ufbx/data/textures/`). To make the example actually demonstrate
//! texture loading, the default asset source is swapped for a router before
//! `DefaultPlugins`: `*.fbx` → `assets/`; anything else → `assets/` first (so
//! PNGs you copy into `assets/textures/` win), falling back to the ufbx corpus.
//! Startup stdout states which source will serve the PNGs and warns loudly when
//! neither location has them (then the cube stays untextured and the loader
//! logs its own "External texture could not be read" warning). After the FBX
//! settles, stdout reports each `textures/*.png` load state plus how many
//! fixture `StandardMaterial`s hold a `base_color_texture`.

use std::f32::consts::PI;
use std::path::Path;

use bevy::asset::LoadState;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
    file::FileAssetReader,
};
use bevy::world_serialization::WorldAsset;
use bevy::{light::CascadeShadowConfigBuilder, prelude::*};
use bevy_ufbx::{FbxAssetLabel, FbxPlugin};

const TEXTURES: &str = "blender_279_internal_textures_7400_binary.fbx";
/// Blender 2.79 fixture reports centimetre-ish extents after conversion — enlarge.
const DEMO_VISUAL_SCALE: f32 = 100.0;

/// The fixture's external texture references (asset-root-relative, `/`-normalized
/// by the loader from Blender's `textures\checkerboard_*.png` RelativeFilename).
const TEXTURE_FILES: [&str; 5] = [
    "textures/checkerboard_diffuse.png",
    "textures/checkerboard_ambient.png",
    "textures/checkerboard_emissive.png",
    "textures/checkerboard_specular.png",
    "textures/checkerboard_weight.png",
];

/// ufbx corpus root (the checkerboard PNGs). Optional at runtime.
const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../libs/ufbx/data");

fn main() {
    let mut app = App::new();

    // External texture bytes are read through the DEFAULT asset source with
    // plain paths, so it must route: `*.fbx` → assets/, everything else →
    // assets/ first, then the ufbx corpus. Must run BEFORE DefaultPlugins
    // builds the AssetServer.
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(|| Box::new(RoutingReader::new())),
    );

    app.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 2000.,
        ..default()
    })
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bevy_ufbx — textures_fbx".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FbxPlugin)
    .add_systems(Startup, setup)
    .add_systems(Update, (rotate, report_when_loaded))
    .run();
}

// ----------------------------------------------------------------------------
// Routing asset source
// ----------------------------------------------------------------------------

/// Serves `*.fbx` from `assets/`, and other files from `assets/` first with a
/// fallback to the ufbx corpus (the fixture's external PNGs).
///
/// Replaces the default source's reader because the loader's texture pre-reads
/// and fallback loads always go through the default source with plain paths.
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
            return self.assets.read(path).await;
        }
        // `assets/` first (user copies in assets/textures/ win), corpus fallback.
        // Inlined (not a helper): every branch must be the SAME concrete reader
        // type to satisfy the single opaque return type.
        match self.assets.read(path).await {
            Ok(reader) => Ok(reader),
            Err(AssetReaderError::NotFound(_)) => self.corpus.read(path).await,
            Err(err) => Err(err),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if is_fbx(path) {
            return self.assets.read_meta(path).await;
        }
        match self.assets.read_meta(path).await {
            Ok(reader) => Ok(reader),
            Err(AssetReaderError::NotFound(_)) => self.corpus.read_meta(path).await,
            Err(err) => Err(err),
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
// Demo
// ----------------------------------------------------------------------------

#[derive(Component)]
struct Spinning;

/// Scene0 handle + probe handles for the fixture's external PNGs (requested so
/// the report can watch their load states; the loader requests the same paths).
#[derive(Resource)]
struct TexturesFixture {
    scene: Handle<WorldAsset>,
    probes: Vec<Handle<Image>>,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assert!(
        Path::new("assets").join(TEXTURES).is_file(),
        "missing assets/{TEXTURES} — copy from ufbx/data"
    );

    println!("textures_fbx — external texture loading (fixture references, not embeds)");
    let in_assets = TEXTURE_FILES
        .iter()
        .all(|file| Path::new("assets").join(file).is_file());
    let in_corpus = TEXTURE_FILES
        .iter()
        .all(|file| Path::new(CORPUS).join(file).is_file());
    if in_assets {
        println!("texture bytes: assets/textures/checkerboard_*.png present — served from assets/");
    } else if in_corpus {
        println!(
            "texture bytes: assets/textures/ absent — falling back to the ufbx corpus at \
             {CORPUS} (checkerboard PNGs come from there)"
        );
    } else {
        eprintln!(
            "WARNING: checkerboard PNGs found neither in assets/textures/ nor {CORPUS}.\n\
             The cube will render UNTEXTURED and the loader will warn that external\n\
             textures could not be read. To fix, copy them from libs/ufbx/data/textures/:\n\
             mkdir -p assets/textures && cp ../../libs/ufbx/data/textures/checkerboard_*.png \
             assets/textures/"
        );
    }

    let scene = asset_server.load(FbxAssetLabel::Scene(0).from_asset(TEXTURES));
    let probes = TEXTURE_FILES
        .iter()
        .map(|file| asset_server.load::<Image>(*file))
        .collect();
    commands.insert_resource(TexturesFixture {
        scene: scene.clone(),
        probes,
    });
    commands.spawn((
        WorldAssetRoot(scene),
        Transform::from_scale(Vec3::splat(DEMO_VISUAL_SCALE)),
        Spinning,
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.25, 0.28, 0.32))),
        Transform::from_xyz(0.0, -0.55, 0.0),
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
        Text::new(
            "textures_fbx — blender_279_internal_textures_7400_binary.fbx (EXTERNAL pngs)\n\
             Look for: checkerboard base-color on the cube (not flat grey).\n\
             Startup stdout says which source serves textures/checkerboard_*.png;\n\
             after load it reports each PNG's LoadState + bound base_color_textures.",
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
}

/// One-shot stdout report once Scene0 and the probe PNG loads settle.
fn report_when_loaded(
    asset_server: Res<AssetServer>,
    fixture: Res<TexturesFixture>,
    materials: Res<Assets<StandardMaterial>>,
    mut reported: Local<bool>,
) {
    if *reported {
        return;
    }
    match asset_server.load_state(&fixture.scene) {
        LoadState::Loaded => {}
        LoadState::Failed(err) => {
            error!("FBX load failed: {err:?}");
            *reported = true;
            return;
        }
        _ => return,
    }
    // Probe requests are registered by the loader run (and mirrored here), so
    // they settle to Loaded/Failed within a frame or two of Scene0 landing.
    if fixture.probes.iter().any(|handle| {
        !matches!(
            asset_server.load_state(handle),
            LoadState::Loaded | LoadState::Failed(_)
        )
    }) {
        return;
    }

    println!("--- external texture bytes (textures/*.png requested by the loader run) ---");
    let mut landed = 0usize;
    for handle in &fixture.probes {
        let state = asset_server.load_state(handle);
        let path = asset_server
            .get_path(handle.id())
            .map(|p| p.path().display().to_string())
            .unwrap_or_default();
        if matches!(state, LoadState::Loaded) {
            landed += 1;
        }
        println!("  {path}: {state:?}");
    }

    // Fixture StandardMaterials (label Material{N}/Standard) that hold a
    // base-color handle. The handle is bound even when the PNG is missing —
    // whether pixels exist is exactly what the states above report.
    let mut total = 0usize;
    let mut bound = 0usize;
    for (id, material) in materials.iter() {
        let Some(path) = asset_server.get_path(id) else {
            continue;
        };
        let Some(label) = path.label() else {
            continue;
        };
        let Some(rest) = label.strip_prefix("Material") else {
            continue;
        };
        if !rest.contains('/') {
            continue; // skip FbxMaterial containers / inverted twins naming
        }
        total += 1;
        if material.base_color_texture.is_some() {
            bound += 1;
        }
    }
    println!(
        "  landed {landed}/{} PNG(s); {bound}/{total} fixture StandardMaterial(s) bound \
              to base_color_texture",
        fixture.probes.len(),
    );
    if landed == 0 {
        println!(
            "  → no external pixels: cube renders UNTEXTURED (see startup WARNING for the \
             fix)"
        );
    } else {
        println!("  → textured cube: base-color pixels served through the routed default source");
    }
    *reported = true;
}

fn rotate(time: Res<Time>, mut q: Query<&mut Transform, With<Spinning>>) {
    for mut t in &mut q {
        t.rotate_y(time.delta_secs() * 0.4);
    }
}
