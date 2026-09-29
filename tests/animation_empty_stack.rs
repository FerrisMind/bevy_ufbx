//! Regression: an FBX whose `AnimationStack` has no curves must not trigger scene animation
//! wiring, and must still produce its labeled (empty) clip.
//!
//! `synthetic_id_collision_7500_ascii.fbx` (ufbx corpus) is a 10,000-deep `Model` chain whose
//! only animation content is a curve-less `AnimationStack` + `AnimationLayer`. Before the fix,
//! one labeled-but-empty clip made `has_animations` true, and `build_scene` then walked the whole
//! ancestor chain for every node (`animation_name_path` + `is_ancestor_of`) — O(nodes x depth),
//! ~5e7 `Name` allocations, minutes of load time in a dev build. The scene gate now keys off real
//! curve presence in `process_animations` (`ProcessedAnimations::has_curves`), while the empty
//! clip keeps its `Animation0` label and its `named_animations` entry.
//!
//! Corpus location: `UFBX_TEST_DATA`, else `<crate>/../../libs/ufbx/data` when it exists;
//! the corpus test skips with a printed reason otherwise.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::animation::{AnimatedBy, AnimationClip, AnimationPlayer};
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxAssetLabel, FbxPlugin};

/// Deep-chain fixture with a single curve-less animation stack.
const EMPTY_STACK_FIXTURE: &str = "synthetic_id_collision_7500_ascii.fbx";
/// Wall-clock budget for one real `AssetServer` load of the fixture. The same load took minutes
/// before the wiring gate; `tests/load_all_ufbx_data.rs` uses the same 15 s per-file budget.
const LOAD_BUDGET: Duration = Duration::from_secs(15);
/// Hard cap so a regression fails the test instead of hanging the suite. It only trips between
/// `app.update()` calls, so a fully blocking load is still bounded by the corpus watchdog in
/// `tests/load_all_ufbx_data.rs`.
const LOAD_HARD_CAP: Duration = Duration::from_secs(60);

/// Crate `assets` directory, absolute so the test does not depend on the process cwd.
fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn asset_app(file_path: impl Into<String>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: file_path.into(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<AnimationClip>()
        .init_asset::<WorldAsset>();
    app
}

/// Wait for a load, returning the wall time it took; `None` when it failed or exceeded the cap.
fn wait_for_load(app: &mut App, handle: &Handle<Fbx>, name: &str) -> Option<Duration> {
    let start = Instant::now();
    loop {
        app.update();
        match app.world().resource::<AssetServer>().load_state(handle) {
            LoadState::Loaded => return Some(start.elapsed()),
            LoadState::Failed(err) => panic!("{name} failed to load: {err}"),
            _ => {}
        }
        if start.elapsed() > LOAD_HARD_CAP {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn corpus_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("UFBX_TEST_DATA") {
        let dir = PathBuf::from(&value);
        assert!(
            dir.is_dir(),
            "UFBX_TEST_DATA is set to {value:?}, which is not a directory"
        );
        return Some(dir);
    }
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/ufbx/data");
    default.is_dir().then_some(default)
}

#[test]
fn empty_stack_loads_fast_and_wires_no_animation_targets() {
    let Some(corpus) = corpus_dir() else {
        println!(
            "skipping empty_stack_loads_fast_and_wires_no_animation_targets: no FBX corpus found. \
             Set UFBX_TEST_DATA to a ufbx test data directory, or place it at ../../libs/ufbx/data."
        );
        return;
    };

    let mut app = asset_app(corpus.to_string_lossy().to_string());
    let (fbx_handle, clip_handle): (Handle<Fbx>, Handle<AnimationClip>) = {
        let server = app.world().resource::<AssetServer>();
        (
            server.load(EMPTY_STACK_FIXTURE),
            server.load(FbxAssetLabel::Animation(0).from_asset(EMPTY_STACK_FIXTURE)),
        )
    };

    let elapsed = wait_for_load(&mut app, &fbx_handle, EMPTY_STACK_FIXTURE)
        .unwrap_or_else(|| panic!("{EMPTY_STACK_FIXTURE} did not load within {LOAD_HARD_CAP:?}"));
    assert!(
        elapsed < LOAD_BUDGET,
        "{EMPTY_STACK_FIXTURE} took {elapsed:?}, over the {LOAD_BUDGET:?} budget — the empty \
         animation stack is probably triggering per-node animation-target wiring again"
    );

    // The labeled (empty) clip contract is preserved: the clip exists, is registered under
    // `Animation0`, and really is curve-less (so the gate below is exercised meaningfully).
    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips
        .get(&clip_handle)
        .expect("Animation0 label should still resolve for a curve-less stack");
    assert!(
        clip.curves().is_empty(),
        "fixture premise changed: the stack now bakes {} curve target(s)",
        clip.curves().len()
    );

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        assert!(
            !fbx.animations.is_empty(),
            "the empty stack must still list a labeled clip on Fbx.animations"
        );
        assert!(
            !fbx.named_animations.is_empty(),
            "the empty stack must still be reachable through named_animations"
        );
        fbx.default_scene.as_ref().expect("default_scene").clone()
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds.get(&scene).expect("Scene0 WorldAsset");
    let mut entities = 0usize;
    let mut players = 0usize;
    let mut animated_by = 0usize;
    for entity in world_asset.world.iter_entities() {
        entities += 1;
        players += usize::from(
            world_asset
                .world
                .get::<AnimationPlayer>(entity.id())
                .is_some(),
        );
        animated_by += usize::from(world_asset.world.get::<AnimatedBy>(entity.id()).is_some());
    }

    assert!(
        entities > 1_000,
        "expected the full 10k-deep hierarchy in the scene, found {entities} entities"
    );
    assert_eq!(
        players, 0,
        "a curve-less clip must not insert AnimationPlayer components"
    );
    assert_eq!(
        animated_by, 0,
        "a curve-less clip must not wire AnimationTargetId/AnimatedBy components"
    );
}

/// The other side of the gate: a genuinely animated stack still wires players and targets.
#[test]
fn animated_fixture_still_wires_players_and_targets() {
    let mut app = asset_app(assets_dir().to_string_lossy().to_string());
    let fbx_handle: Handle<Fbx> = app.world().resource::<AssetServer>().load("cube_anim.fbx");

    let elapsed =
        wait_for_load(&mut app, &fbx_handle, "cube_anim.fbx").expect("cube_anim.fbx should load");
    assert!(elapsed < LOAD_BUDGET);

    let scene = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.default_scene.as_ref().expect("default_scene").clone()
    };

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world_asset = worlds.get(&scene).expect("Scene0 WorldAsset");
    let mut players = 0usize;
    let mut animated_by = 0usize;
    for entity in world_asset.world.iter_entities() {
        players += usize::from(
            world_asset
                .world
                .get::<AnimationPlayer>(entity.id())
                .is_some(),
        );
        animated_by += usize::from(world_asset.world.get::<AnimatedBy>(entity.id()).is_some());
    }

    assert!(
        players > 0,
        "an animated fixture must still receive AnimationPlayer wiring"
    );
    assert!(
        animated_by > 0,
        "an animated fixture must still receive AnimationTargetId/AnimatedBy wiring"
    );
}
