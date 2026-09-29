//! Integration regression for stepped (constant) FBX tangents.
//!
//! `ufbx::bake_anim` encodes stepped tangents as samples 1 ms apart carrying
//! `STEP_LEFT`/`STEP_RIGHT` flags. The loader must keep the step exact instead of letting
//! those ±1 ms samples decay into linear ramps.
//!
//! Fixture provenance: copied verbatim from the ufbx test corpus
//! `libs/ufbx/data/maya_anim_linear_7700_ascii.fbx`
//! (md5 `4bcfe71710dbb66f3cba44b729faa723`, 17 157 bytes), which backs ufbx's own
//! baked-step expectations in `test/test_animation.h` (`maya_anim_linear_default`).

use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::animation::animation_curves::AnimationCurve;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxPlugin};

/// One node (`pCube1`) with a translation spike at `t = 0.5 s`: a `ConstantStandard` key
/// steps up to the spike value at the key time, and a `ConstantNext` key steps back down
/// immediately after.
const STEPPED_FIXTURE: &str = "maya_anim_linear_7700_ascii.fbx";

/// Model-space value reached by the stepped channel exactly at `t = 0.5 s`.
const STEP_TIME: f32 = 0.5;

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: "assets".into(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<AnimationClip>()
        .init_asset::<WorldAsset>();
    app
}

fn wait_for_load(app: &mut App, handle: &Handle<Fbx>, max_frames: usize) -> bool {
    for _ in 0..max_frames {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.load_state(handle) {
            LoadState::Loaded => return true,
            LoadState::Failed(_) => return false,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    false
}

fn sample_vec3(curve: &dyn AnimationCurve, t: f32) -> Option<Vec3> {
    curve.sample_clamped(t).downcast_ref::<Vec3>().copied()
}

#[test]
fn stepped_tangents_hold_exactly_through_the_loader() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load(STEPPED_FIXTURE)
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "{STEPPED_FIXTURE} failed to load"
    );

    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.animations.first().cloned().expect("baked clip")
    };
    let clip = clips.get(&clip_handle).expect("Animation0 clip");
    assert!(clip.duration() > 0.0, "baked clip should have a duration");

    let mut checked = 0usize;
    for target in clip.curves().keys().copied().collect::<Vec<_>>() {
        for curve in clip.curves_for_target(target).expect("target curves") {
            let curve: &dyn AnimationCurve = &*curve.0;
            let Some(before) = sample_vec3(curve, 0.4) else {
                continue;
            };
            let Some(at_key) = sample_vec3(curve, STEP_TIME) else {
                continue;
            };
            let spike = (at_key - before).length();
            if spike < 1e-5 {
                // Constant channel (e.g. scale) or a channel that does not step.
                continue;
            }
            checked += 1;

            // The held value must stay exact right up to (and right after) the step key.
            // A linear ramp across the ±1 ms bake samples would already have moved ~99.9%
            // of the spike at these times.
            let hold_before = sample_vec3(curve, STEP_TIME - 0.0005).expect("Vec3 sample");
            assert!(
                (hold_before - before).length() < 1e-6,
                "held value must not ramp before the step key: {hold_before:?} vs {before:?}"
            );
            let hold_after = sample_vec3(curve, STEP_TIME + 0.0005).expect("Vec3 sample");
            assert!(
                (hold_after - before).length() < 1e-6,
                "held value must return exactly after the step key: {hold_after:?} vs {before:?}"
            );

            // The spike value is reached at the key time only.
            assert!((at_key - before).length() > 0.5 * spike);
            let after = sample_vec3(curve, 0.52).expect("Vec3 sample");
            assert!(
                (after - before).length() < 1e-6,
                "step must not leak past the spike: {after:?} vs {before:?}"
            );
        }
    }

    assert!(
        checked >= 1,
        "expected one stepped translation channel in {STEPPED_FIXTURE}"
    );
}
