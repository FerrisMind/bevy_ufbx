//! Cubic (Hermite) animation path tests.
//!
//! Layers:
//! 1. Layout probes that pin down Bevy's cubic curve semantics
//!    (`CubicKeyframeCurve`, `CubicRotationCurve`, `WideCubicKeyframeCurve`)
//!    — including two confirmed defects in bevy_animation 0.19.1's wide cubic
//!    curve that force the loader's private `WideHermiteCurve` for morph
//!    weights.
//! 2. End-to-end assertions that cubic-authored FBX clips produce cubic curves
//!    and that stepped clips keep the baked fallback.

use bevy::animation::AnimationClip;
use bevy::animation::gltf_curves::{
    CubicKeyframeCurve, CubicRotationCurve, WideCubicKeyframeCurve,
};
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::math::curve::Curve;
use bevy::math::curve::iterable::IterableCurve;
use bevy::math::{Quat, Vec3, Vec4};
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxPlugin};
use std::time::Duration;

// ── Bevy cubic curve layout probes ──────────────────────────────────────────

/// Layout probe for the narrow `CubicKeyframeCurve<Vec3>`: per key the triple
/// is `[in_tangent, value, out_tangent]` and the sampler must use
/// `hermite(v0, out0, in1, v1)` between keys.
#[test]
fn probe_cubic_keyframe_curve_layout() {
    let times = vec![0.0f32, 1.0];
    // key0: in=10 (unused as first key), value=0, out=0.25 (slope d/dt)
    // key1: in=-7 (slope), value=1, out=20 (unused as last key)
    let values = vec![
        Vec3::splat(10.0),
        Vec3::ZERO,
        Vec3::splat(0.25),
        Vec3::splat(-7.0),
        Vec3::ONE,
        Vec3::splat(20.0),
    ];
    let curve = CubicKeyframeCurve::new(times, values).expect("curve");

    // Exact key samples defer to the value slot of the triple.
    let at0 = curve.sample_clamped(0.0);
    let at1 = curve.sample_clamped(1.0);
    assert!(
        (at0 - Vec3::ZERO).length() < 1e-6,
        "t=0 must return the value slot, got {at0:?}"
    );
    assert!(
        (at1 - Vec3::ONE).length() < 1e-6,
        "t=1 must return the value slot, got {at1:?}"
    );

    // hermite(v0=0, m0=0.25, v1=1, m1=-7) at s=0.5:
    // h00=0.5, h10=0.125, h01=0.5, h11=-0.125
    // = 0 + 0.125*0.25 + 0.5*1 + (-0.125)*(-7) = 1.40625
    let mid = curve.sample_clamped(0.5);
    assert!(
        (mid.x - 1.40625).abs() < 1e-5,
        "midpoint must follow hermite(v0, out0, in1, v1), got {} (expected 1.40625)",
        mid.x
    );
}

/// Layout probe for `CubicRotationCurve`: same triple layout over `Vec4`,
/// sampled value normalized to a `Quat`.
#[test]
fn probe_cubic_rotation_curve_layout() {
    let times = vec![0.0f32, 1.0];
    let q0 = Quat::IDENTITY;
    let q1 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    let values = vec![
        Vec4::from(q0),       // in (unused as first key)
        Vec4::from(q0),       // value
        Vec4::ZERO,           // out slope: stay at identity initially
        Vec4::from(q1) * 0.2, // in slope at key1
        Vec4::from(q1),       // value
        Vec4::ZERO,           // out (unused as last key)
    ];
    let curve = CubicRotationCurve::new(times, values).expect("curve");

    let at0 = curve.sample_clamped(0.0);
    assert!(
        at0.dot(q0).abs() > 0.9999,
        "t=0 must be identity, got {at0:?}"
    );
    let at1 = curve.sample_clamped(1.0);
    assert!(at1.dot(q1).abs() > 0.9999, "t=1 must be q1, got {at1:?}");
}

/// bevy_animation 0.19.1 defect probe 1: sampling `WideCubicKeyframeCurve`
/// exactly at a key time (which every animation start hits, `t = 0`) panics —
/// the exact branch slices `v[width..width*2]` with the full `3N` width on a
/// `3N` chunk. This is why morph weights use the loader's private
/// `WideHermiteCurve` instead.
#[test]
#[should_panic(expected = "out of range")]
fn probe_wide_cubic_panics_when_sampled_at_a_key_time() {
    let times = vec![0.0f32, 1.0];
    let values = vec![10.0f32, 0.0, 0.25, -7.0, 1.0, 20.0];
    let curve = WideCubicKeyframeCurve::new(times, values).expect("curve");
    let _sampled: Vec<f32> = curve.sample_iter_clamped(0.0).collect();
}

/// bevy_animation 0.19.1 defect probe 2: the between-key branch passes the
/// end value and end tangent swapped, so a segment that must read
/// `hermite(0, 0.25, -7, 1) = 1.40625` instead evaluates to `-3.59375`.
///
/// When bevy fixes this upstream (and the panic probe above starts failing),
/// the morph path can switch back to `WideCubicKeyframeCurve` directly.
#[test]
fn probe_wide_cubic_between_key_arguments_are_swapped() {
    let times = vec![0.0f32, 1.0];
    let values = vec![10.0f32, 0.0, 0.25, -7.0, 1.0, 20.0];
    let curve = WideCubicKeyframeCurve::new(times, values).expect("curve");
    let mid: Vec<f32> = curve.sample_iter_clamped(0.5).collect();
    assert_eq!(mid.len(), 1, "one channel must be sampled");
    assert!(
        (mid[0] - (-3.59375)).abs() < 1e-6,
        "expected the swapped-argument result -3.59375 (upstream defect), got {}. \
         If bevy fixed the argument order this probe should be retired and the morph \
         path switched to WideCubicKeyframeCurve.",
        mid[0]
    );
}

// ── End-to-end: cubic FBX clips produce cubic curves ───────────────────────

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

/// Debug strings of every curve in the fixture's first baked clip.
fn clip_curve_debug(app: &mut App, fbx_handle: &Handle<Fbx>) -> Vec<String> {
    let clip_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(fbx_handle).expect("Fbx asset");
        fbx.animations.first().cloned().expect("baked clip")
    };
    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips.get(&clip_handle).expect("AnimationClip asset");
    let mut debugs = Vec::new();
    for curves in clip.curves().values() {
        for curve in curves {
            debugs.push(format!("{curve:?}"));
        }
    }
    debugs
}

/// `cube_anim.fbx` authors every TRS channel as `Cubic|TangeantAuto`, so the
/// clip must contain Bevy cubic curves (not the resampled linear fallback).
#[test]
fn cube_anim_clip_contains_cubic_trs_curves() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("cube_anim.fbx")
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "cube_anim.fbx failed to load"
    );

    let debugs = clip_curve_debug(&mut app, &fbx_handle);
    assert!(!debugs.is_empty(), "clip must contain curves");
    assert!(
        debugs.iter().any(|d| d.contains("CubicKeyframeCurve")),
        "expected a CubicKeyframeCurve (translation/scale); curves: {debugs:#?}"
    );
    assert!(
        debugs.iter().any(|d| d.contains("CubicRotationCurve")),
        "expected a CubicRotationCurve (rotation); curves: {debugs:#?}"
    );
}

/// Regression: a stepped/constant fixture must stay on the baked fallback —
/// no cubic curves may appear.
#[test]
fn stepped_clip_keeps_baked_fallback_curves() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("maya_anim_linear_7700_ascii.fbx")
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "maya_anim_linear_7700_ascii.fbx failed to load"
    );

    let debugs = clip_curve_debug(&mut app, &fbx_handle);
    assert!(!debugs.is_empty(), "clip must contain curves");
    assert!(
        !debugs
            .iter()
            .any(|d| d.contains("CubicKeyframeCurve") || d.contains("CubicRotationCurve")),
        "stepped clip must not contain cubic curves: {debugs:#?}"
    );
}

/// `blend_shape_cube.fbx` authors `DeformPercent` as cubic keys, so its morph
/// track must be the loader's wide Hermite curve — and must be sampleable at
/// key times without the upstream `WideCubicKeyframeCurve` panic.
#[test]
fn blend_shape_cube_clip_contains_wide_hermite_morph_curve() {
    let mut app = headless_app();
    let fbx_handle: Handle<Fbx> = {
        let server = app.world().resource::<AssetServer>();
        server.load("blend_shape_cube.fbx")
    };
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "blend_shape_cube.fbx failed to load"
    );

    let debugs = clip_curve_debug(&mut app, &fbx_handle);
    assert!(!debugs.is_empty(), "clip must contain curves");
    assert!(
        debugs.iter().any(|d| d.contains("WideHermiteCurve")),
        "expected the wide Hermite morph curve; curves: {debugs:#?}"
    );

    // Sample every curve at its domain start: the morph curve must survive
    // key-time sampling (the upstream WideCubic panic would abort here).
    let clip_handle = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(&fbx_handle).expect("Fbx asset");
        fbx.animations.first().cloned().expect("baked clip")
    };
    let clips = app.world().resource::<Assets<AnimationClip>>();
    let clip = clips.get(&clip_handle).expect("AnimationClip asset");
    for curves in clip.curves().values() {
        for curve in curves {
            let _sampled = curve.0.sample_clamped(0.0);
        }
    }
}
