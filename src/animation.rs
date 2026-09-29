//! Bake FBX animation stacks into Bevy [`AnimationClip`]s.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::{animation_name_path, node_typed_id};
use crate::utils::convert_transform;
use bevy::animation::animation_curves::WeightsCurve;
use bevy::animation::gltf_curves::{
    CubicKeyframeCurve, CubicRotationCurve, WideLinearKeyframeCurve,
};
use bevy::animation::prelude::{AnimatableCurve, AnimatableProperty};
use bevy::animation::{AnimationClip, AnimationTargetId, animated_field};
use bevy::asset::{Handle, LoadContext};
use bevy::math::curve::iterable::IterableCurve;
use bevy::math::curve::{ConstantCurve, Curve, Interval, UnevenSampleAutoCurve};
use bevy::mesh::morph::MAX_MORPH_WEIGHTS;
use bevy::prelude::*;
use bevy::reflect::Reflect;
use std::collections::{HashMap, HashSet};

/// Result of animation baking.
pub struct ProcessedAnimations {
    pub animations: Vec<Handle<AnimationClip>>,
    pub named_animations: HashMap<Box<str>, Handle<AnimationClip>>,
    /// True when at least one baked clip actually contains a curve.
    ///
    /// Curve-less `ufbx` anim stacks still produce a labeled (empty) clip — that contract is
    /// preserved — but a clip with no curves can never drive anything, so scene animation
    /// wiring keys off this flag instead of `!animations.is_empty()`. Without it an empty
    /// stack makes `build_scene` walk the ancestor chain of every node
    /// (`animation_name_path` / `is_ancestor_of`), which is O(nodes x depth) — minutes on a
    /// 10,000-deep chain (`synthetic_id_collision_7500_ascii.fbx`).
    pub has_curves: bool,
}

/// Bake each FBX anim stack into a labeled [`AnimationClip`].
///
/// Clips are labeled `Animation{N}` only. Bevy's [`LoadContext`] does not support
/// registering a second label for the same asset, so stack names are exposed via
/// [`ProcessedAnimations::named_animations`] (and [`crate::Fbx::named_animations`])
/// rather than as `AssetPath` labels.
///
/// When `generate_rest` is true, an additional clip labeled `AnimationRest` /
/// named `"Rest"` is appended with a single keyframe at `t = 0` for each node
/// that appeared in any baked stack (rest local transform + morph weights).
pub fn process_animations(
    scene: &ufbx::Scene,
    load_context: &mut LoadContext,
    bake_fps: f32,
    generate_rest: bool,
) -> Result<ProcessedAnimations, FbxError> {
    let mut animations = Vec::new();
    let mut named_animations = HashMap::new();
    let mut animated_typed_ids = HashSet::new();
    // Set below whenever a clip with at least one curve is produced (TRS or morph weights).
    let mut has_curves = false;
    let resample_rate = f64::from(bake_fps.max(0.0));

    for (stack_index, stack) in scene.anim_stacks.as_ref().iter().enumerate() {
        // Trim so keys match DCC take names like "Take 001" without stray whitespace.
        let take_name = {
            let raw = stack.element.name.trim();
            if raw.is_empty() {
                format!("take{stack_index}")
            } else {
                raw.to_string()
            }
        };

        let baked = ufbx::bake_anim(
            scene,
            &stack.anim,
            ufbx::BakeOpts {
                trim_start_time: true,
                resample_rate,
                ..Default::default()
            },
        )
        .map_err(|e| {
            FbxError::ConversionError(format!(
                "Failed to bake animation stack '{take_name}': {e:?}"
            ))
        })?;

        let mut clip = AnimationClip::default();
        let duration = baked.playback_duration as f32;
        if duration.is_finite() {
            clip.set_duration(duration);
        } else {
            warn!(
                "FBX animation stack '{take_name}': baked playback duration {} is not finite; \
                 clip duration left at 0",
                baked.playback_duration
            );
        }

        // Exact-cubic prerequisites shared by the TRS and morph paths; any
        // `None` here means the baked/resampled tracks are used unchanged.
        let shift = clip_time_shift(&stack.anim);
        let exact_layer = exact_anim_layer(&stack.anim);

        for baked_node in baked.nodes.as_ref().iter() {
            animated_typed_ids.insert(baked_node.typed_id);
            let path = animation_name_path(scene, baked_node.typed_id);
            if path.is_empty() {
                continue;
            }
            let target_id = AnimationTargetId::from_names(path.iter());
            add_trs_curves(
                &mut clip,
                target_id,
                baked_node,
                &path,
                scene,
                &stack.anim,
                exact_layer,
                shift,
            );
        }

        add_morph_weight_curves(
            scene,
            &stack.anim,
            &baked,
            &mut clip,
            bake_fps,
            exact_layer,
            shift,
        );

        has_curves |= !clip.curves().is_empty();

        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Animation(stack_index).to_string(), clip);
        named_animations.insert(Box::from(take_name.as_str()), handle.clone());
        animations.push(handle);
    }

    if generate_rest {
        let (rest_handle, rest_has_curves) =
            add_rest_animation_clip(scene, &animated_typed_ids, load_context);
        has_curves |= rest_has_curves;
        named_animations.insert(Box::from("Rest"), rest_handle.clone());
        animations.push(rest_handle);
    }

    Ok(ProcessedAnimations {
        animations,
        named_animations,
        has_curves,
    })
}

/// Rest/bind clip: constant TRS (+ morph weights) at `t = 0` for animated nodes.
///
/// Returns the clip handle and whether the clip received any curve (false when no node was
/// animated in any baked stack, so even the rest clip is empty).
fn add_rest_animation_clip(
    scene: &ufbx::Scene,
    animated_typed_ids: &HashSet<u32>,
    load_context: &mut LoadContext,
) -> (Handle<AnimationClip>, bool) {
    let mut clip = AnimationClip::default();
    clip.set_duration(0.0);

    let mut typed_ids: Vec<u32> = animated_typed_ids.iter().copied().collect();
    typed_ids.sort_unstable();

    for typed_id in typed_ids {
        let Some(node) = scene.nodes.get(typed_id as usize) else {
            continue;
        };
        if node.is_root {
            continue;
        }
        let path = animation_name_path(scene, typed_id);
        if path.is_empty() {
            continue;
        }
        let target_id = AnimationTargetId::from_names(path.iter());
        let transform = convert_transform(&node.local_transform);
        add_constant_trs(&mut clip, target_id, &transform);
        add_rest_morph_weights(node, target_id, &mut clip);
    }

    let has_curves = !clip.curves().is_empty();
    let handle = load_context.add_labeled_asset(FbxAssetLabel::AnimationRest.to_string(), clip);
    (handle, has_curves)
}

fn add_constant_trs(clip: &mut AnimationClip, target_id: AnimationTargetId, transform: &Transform) {
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::translation),
            ConstantCurve::new(Interval::EVERYWHERE, transform.translation),
        ),
    );
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::rotation),
            ConstantCurve::new(Interval::EVERYWHERE, transform.rotation.normalize()),
        ),
    );
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::scale),
            ConstantCurve::new(Interval::EVERYWHERE, transform.scale),
        ),
    );
}

fn add_rest_morph_weights(
    node: &ufbx::Node,
    target_id: AnimationTargetId,
    clip: &mut AnimationClip,
) {
    let Some(mesh_ref) = node.mesh.as_ref() else {
        return;
    };
    let mesh = mesh_ref.as_ref();
    if mesh.blend_deformers.is_empty() {
        return;
    }

    let mut weights: Vec<f32> = Vec::new();
    for deformer in mesh.blend_deformers.as_ref().iter() {
        for channel in deformer.channels.as_ref().iter() {
            if channel.target_shape.is_none() {
                continue;
            }
            if weights.len() >= MAX_MORPH_WEIGHTS {
                break;
            }
            weights.push(channel.weight as f32);
        }
    }
    if weights.is_empty() {
        return;
    }

    clip.add_curve_to_target(
        target_id,
        WeightsCurve(ConstantCurve::new(Interval::EVERYWHERE, weights)),
    );
}

/// Step semantics of a single ufbx baked key.
///
/// `ufbx::bake_anim` (its default step handling) has *already* expanded stepped tangents
/// into adjacent samples: a key at `t - 0.001` holds the previous value (`STEP_LEFT`) and a
/// key at `t + 0.001` carries the following value (`STEP_RIGHT`). `STEP_KEY` marks the key
/// the step belongs to and does not change interpolation. Rewriting these flags into extra
/// hold samples would double-expand the track; instead the flags are interpreted exactly as
/// ufbx's own `ufbx_evaluate_baked_vec3` / `ufbx_evaluate_baked_quat` do, so the ±1 ms
/// samples never decay into ramps.
#[derive(Clone, Copy, Debug, Default, Reflect)]
struct KeyStep {
    /// `STEP_LEFT`: hold this key's value for the segment *after* it.
    left: bool,
    /// `STEP_RIGHT`: take this key's value for the segment *before* it.
    right: bool,
}

/// Extract the step semantics of one baked key.
fn key_step(flags: ufbx::BakedKeyFlags) -> KeyStep {
    KeyStep {
        left: flags.has_any(ufbx::BakedKeyFlags::STEP_LEFT),
        right: flags.has_any(ufbx::BakedKeyFlags::STEP_RIGHT),
    }
}

/// A baked track after filtering: finite, strictly increasing sample times.
#[derive(Clone, Debug, Default)]
struct CleanTrack<T> {
    times: Vec<f32>,
    values: Vec<T>,
    steps: Vec<KeyStep>,
    /// Keys skipped because a time/value was not finite, or because two `f64` times
    /// collapsed to the same `f32` time.
    dropped: usize,
}

impl<T> CleanTrack<T> {
    fn sample_count(&self) -> usize {
        self.times.len()
    }

    fn has_steps(&self) -> bool {
        self.steps.iter().any(|step| step.left || step.right)
    }

    fn push(&mut self, time: f32, value: T, step: KeyStep) {
        self.times.push(time);
        self.values.push(value);
        self.steps.push(step);
    }
}

/// Convert translation/scale keys into finite `f32` samples, dropping bad ones.
fn clean_vec3_keys(keys: &[ufbx::BakedVec3]) -> CleanTrack<Vec3> {
    let mut track = CleanTrack::default();
    for key in keys {
        let time = key.time as f32;
        let value = Vec3::new(key.value.x as f32, key.value.y as f32, key.value.z as f32);
        let strictly_later = track.times.last().is_none_or(|last| time > *last);
        if !time.is_finite() || !value.is_finite() || !strictly_later {
            track.dropped += 1;
            continue;
        }
        track.push(time, value, key_step(key.flags));
    }
    track
}

/// Convert rotation keys into normalized quaternions with antipodal continuity.
///
/// `ufbx::bake_anim` already fixes antipodality of its own output, but the loader must stay
/// correct for any baked key list: adjacent keys on opposite quaternion hemispheres would
/// otherwise take the long way around when Bevy blends rotations. Each key is fixed against
/// the previous *kept* key with [`ufbx::quat_fix_antipodal`], then normalized.
fn clean_rotation_keys(keys: &[ufbx::BakedQuat]) -> CleanTrack<Quat> {
    let mut track = CleanTrack::default();
    let mut previous: Option<ufbx::Quat> = None;
    for key in keys {
        let time = key.time as f32;
        let raw = match previous {
            Some(prev) => ufbx::quat_fix_antipodal(key.value, prev),
            None => key.value,
        };
        let value =
            Quat::from_xyzw(raw.x as f32, raw.y as f32, raw.z as f32, raw.w as f32).normalize();
        let strictly_later = track.times.last().is_none_or(|last| time > *last);
        if !time.is_finite() || !value.is_finite() || !strictly_later {
            track.dropped += 1;
            continue;
        }
        previous = Some(raw);
        track.push(time, value, key_step(key.flags));
    }
    track
}

/// Domain of a cleaned track; `times` is finite and strictly increasing.
fn track_domain(times: &[f32]) -> Interval {
    let last = times.len() - 1;
    Interval::new(times[0], times[last]).unwrap_or(Interval::EVERYWHERE)
}

/// Sample a cleaned baked track exactly like ufbx samples its own baked keys.
///
/// * `STEP_LEFT` on the previous key holds its value across the whole segment.
/// * `STEP_RIGHT` on the next key takes its value across the whole segment.
/// * anything else interpolates (`lerp` for vectors, `slerp` for rotations).
fn sample_baked<T: Clone>(
    times: &[f32],
    values: &[T],
    steps: &[KeyStep],
    t: f32,
    interpolate: impl Fn(&T, &T, f32) -> T,
) -> T {
    debug_assert_eq!(times.len(), values.len());
    debug_assert_eq!(times.len(), steps.len());

    // `!(t > first)` also covers NaN, which would break the partition point below.
    if !(t > times[0]) {
        return values[0].clone();
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return values[last].clone();
    }

    let index = times.partition_point(|time| *time <= t) - 1;
    if times[index] == t {
        return values[index].clone();
    }
    if steps[index].left {
        return values[index].clone();
    }
    if steps[index + 1].right {
        return values[index + 1].clone();
    }
    let span = times[index + 1] - times[index];
    let ratio = if span > 0.0 {
        (t - times[index]) / span
    } else {
        0.0
    };
    interpolate(&values[index], &values[index + 1], ratio)
}

/// Step-aware curve over baked translation/scale keys (`Vec3`).
///
/// Only used when the track carries step flags; step-free tracks keep using
/// [`UnevenSampleAutoCurve`].
#[derive(Clone, Debug, Reflect)]
struct BakedVec3Curve {
    times: Vec<f32>,
    values: Vec<Vec3>,
    steps: Vec<KeyStep>,
}

impl BakedVec3Curve {
    /// Build from a cleaned track; `None` when fewer than two keys remain.
    fn new(track: CleanTrack<Vec3>) -> Option<Self> {
        if track.sample_count() < 2 {
            return None;
        }
        Some(Self {
            times: track.times,
            values: track.values,
            steps: track.steps,
        })
    }
}

impl Curve<Vec3> for BakedVec3Curve {
    fn domain(&self) -> Interval {
        track_domain(&self.times)
    }

    fn sample_unchecked(&self, t: f32) -> Vec3 {
        sample_baked(&self.times, &self.values, &self.steps, t, |a, b, s| {
            a.lerp(*b, s)
        })
    }
}

/// Step-aware curve over baked rotation keys (`Quat`).
#[derive(Clone, Debug, Reflect)]
struct BakedQuatCurve {
    times: Vec<f32>,
    values: Vec<Quat>,
    steps: Vec<KeyStep>,
}

impl BakedQuatCurve {
    /// Build from a cleaned track; `None` when fewer than two keys remain.
    fn new(track: CleanTrack<Quat>) -> Option<Self> {
        if track.sample_count() < 2 {
            return None;
        }
        Some(Self {
            times: track.times,
            values: track.values,
            steps: track.steps,
        })
    }
}

impl Curve<Quat> for BakedQuatCurve {
    fn domain(&self) -> Interval {
        track_domain(&self.times)
    }

    fn sample_unchecked(&self, t: f32) -> Quat {
        sample_baked(&self.times, &self.values, &self.steps, t, |a, b, s| {
            a.slerp(*b, s)
        })
    }
}

/// Warn about a dropped/broken track, naming the animated node path and the track.
fn warn_track(node_path: &[Name], track: &str, reason: impl core::fmt::Display) {
    let path = node_path
        .iter()
        .map(Name::as_str)
        .collect::<Vec<_>>()
        .join("/");
    warn!("FBX animation: {track} curve for '{path}' was dropped: {reason}");
}

// ── Exact cubic path ────────────────────────────────────────────────────────
//
// ufbx keyframes carry per-key cubic tangents (`Keyframe { left, right }`, a
// `Tangent { dx, dy }` pair per `ufbx-0.9.0/src/generated.rs`), but those
// tangents are authored-space data: unit/axis conversion, rotation order and
// static composition happen inside `ufbx_evaluate_transform`, so raw tangents
// cannot be dropped into a Bevy curve as-is. Instead the loader samples ufbx's
// own evaluator at the authored key times (the same function `bake_anim`
// calls per key) and derives per-key derivatives numerically with a
// cubic-exact 4-point stencil. Every segment is then verified against ufbx at
// interior probe times; on any mismatch the channel falls back to the baked
// resampled track unchanged.
//
// The resulting curves are Bevy/gltf-style time-linear Hermite cubics
// (`CubicKeyframeCurve` / `CubicRotationCurve`, wide layout for morph
// weights), matching what `bevy_gltf` builds for glTF `CUBICSPLINE`.

/// Shortest authored span accepted for stencil/verification sampling; below
/// this an `f32` key time cannot resolve the four stencil points.
const EXACT_MIN_SEGMENT: f64 = 1e-3;

/// Clip-time trim mirroring ufbx's `trim_start_time` bake option
/// (`ktime_offset` in `ufbx.c`): keys move left by `anim.time_begin` only
/// when the stack starts after zero.
fn clip_time_shift(anim: &ufbx::Anim) -> f64 {
    if anim.time_begin > 0.0 {
        anim.time_begin
    } else {
        0.0
    }
}

/// The single animation layer the exact path may read key *times* from, or
/// `None` when the baked tracks must be used.
///
/// With one layer the composed values `ufbx_evaluate_transform` returns are
/// exactly that layer's curves, so unioned key times cover every wiggle.
/// Multi-layer blends can add key times this function would never see, and
/// user overrides can change values behind the curves' back.
fn exact_anim_layer(anim: &ufbx::Anim) -> Option<&ufbx::AnimLayer> {
    let layers = anim.layers.as_ref();
    if layers.len() != 1 {
        return None;
    }
    if !anim.transform_overrides.is_empty() || !anim.prop_overrides.is_empty() {
        return None;
    }
    Some(layers[0].as_ref())
}

/// Structural guard: `ufbx_evaluate_transform` on a node must not read other
/// nodes' animated scaling (scale-helper / componentwise-inherit machinery),
/// otherwise its values differ from what `bake_anim` records for the same
/// node (bake evaluates with `IGNORE_SCALE_HELPER | IGNORE_COMPONENTWISE_SCALE`
/// and re-applies helper keys itself, `ufbx.c` bake node eval).
fn exact_node_guard(node: &ufbx::Node) -> bool {
    !node.is_root
        && node.scale_helper.is_none()
        && node.inherit_scale_node.is_none()
        && !node.is_scale_helper
        && node.parent.as_ref().is_none_or(|parent| {
            parent.scale_helper.is_none() && parent.inherit_scale_node.is_none()
        })
}

/// Union of authored key times across the given curves, plus whether any span
/// is authored `Cubic`.
///
/// Returns `None` when a curve repeats/mirrors outside its key range
/// (extrapolation would add structure the union never sees) or when any span
/// is stepped (`ConstantPrev`/`ConstantNext`) — those channels keep the baked
/// step-aware fallback. Span classification uses the *left* key of a span,
/// exactly like `ufbx_evaluate_curve_flags` in `ufbx.c`.
fn union_cubic_key_times<'a>(
    curves: impl IntoIterator<Item = &'a ufbx::AnimCurve>,
) -> Option<(Vec<f64>, bool)> {
    let mut times: Vec<f64> = Vec::new();
    let mut saw_cubic = false;
    for curve in curves {
        if curve.pre_extrapolation.mode != ufbx::ExtrapolationMode::Constant
            || curve.post_extrapolation.mode != ufbx::ExtrapolationMode::Constant
        {
            return None;
        }
        let keys = curve.keyframes.as_ref();
        for span in keys.windows(2) {
            match span[0].interpolation {
                ufbx::Interpolation::Linear => {}
                ufbx::Interpolation::Cubic => saw_cubic = true,
                ufbx::Interpolation::ConstantPrev | ufbx::Interpolation::ConstantNext => {
                    return None;
                }
            }
        }
        times.extend(keys.iter().map(|key| key.time));
    }
    Some((times, saw_cubic))
}

/// Sort/dedup a key-time union and require a usable strictly-increasing
/// cubic series (at least two keys, one of them authored cubic).
fn finish_key_times(mut times: Vec<f64>, saw_cubic: bool) -> Option<Vec<f64>> {
    if !saw_cubic || times.len() < 2 {
        return None;
    }
    if times.iter().any(|time| !time.is_finite()) {
        return None;
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    times.dedup();
    if times.len() < 2 || times.windows(2).any(|pair| pair[1] <= pair[0]) {
        return None;
    }
    Some(times)
}

/// Union of authored key times for one transform property of a node, or
/// `None` when the property is absent, not cubic, stepped, or otherwise
/// unsuitable for the exact path.
fn node_channel_key_times(
    layer: &ufbx::AnimLayer,
    element: &ufbx::Element,
    prop: &str,
) -> Option<Vec<f64>> {
    let anim_prop = layer.find_anim_prop(element, prop)?;
    let (times, saw_cubic) =
        union_cubic_key_times(anim_prop.anim_value.curves.iter().flatten().map(|c| &**c))?;
    finish_key_times(times, saw_cubic)
}

/// Cubic-exact forward derivative stencil: `f'(t0)` from `t0, t0+h, t0+2h,
/// t0+3h`. Exact for cubic polynomials (the shape of a `Cubic` ufbx span with
/// auto tangents), `O(h^4)` otherwise.
fn stencil_forward(samples: &[f64; 4], h: f64) -> f64 {
    (-11.0 / 6.0 * samples[0] + 3.0 * samples[1] - 1.5 * samples[2] + samples[3] / 3.0) / h
}

/// Cubic-exact backward derivative stencil: `f'(t1)` from `t1, t1-h, t1-2h,
/// t1-3h` (mirror of [`stencil_forward`]).
fn stencil_backward(samples: &[f64; 4], h: f64) -> f64 {
    (11.0 / 6.0 * samples[0] - 3.0 * samples[1] + 1.5 * samples[2] - samples[3] / 3.0) / h
}

/// Time-linear cubic Hermite, the exact semantics of Bevy's
/// `cubic_spline_interpolation` (tangents are slopes `d(value)/d(time)`).
fn hermite_f64(
    value_start: f64,
    slope_out: f64,
    value_end: f64,
    slope_in: f64,
    s: f64,
    dt: f64,
) -> f64 {
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    h00 * value_start + h10 * dt * slope_out + h01 * value_end + h11 * dt * slope_in
}

/// Which transform channel is being built (selects the raw property and the
/// value extracted from [`ufbx::evaluate_transform`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrsChannel {
    Translation,
    Rotation,
    Scale,
}

impl TrsChannel {
    fn prop(self) -> &'static str {
        match self {
            TrsChannel::Translation => "Lcl Translation",
            TrsChannel::Rotation => "Lcl Rotation",
            TrsChannel::Scale => "Lcl Scaling",
        }
    }
}

/// Sample one channel at raw (untrimmed) ufbx time. `reference` is the nearest
/// fixed key for rotation continuity: quaternion evaluations are normalized
/// and antipodal-fixed against it so stencils differentiate a continuous lift.
/// The fourth component stays `0.0` for `Vec3` channels.
fn eval_channel(
    anim: &ufbx::Anim,
    node: &ufbx::Node,
    time: f64,
    channel: TrsChannel,
    reference: Option<&[f64; 4]>,
) -> Option<[f64; 4]> {
    let transform = ufbx::evaluate_transform(anim, node, time);
    match channel {
        TrsChannel::Translation => {
            let v = [
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
                0.0,
            ];
            v.iter().all(|x| x.is_finite()).then_some(v)
        }
        TrsChannel::Scale => {
            let v = [transform.scale.x, transform.scale.y, transform.scale.z, 0.0];
            v.iter().all(|x| x.is_finite()).then_some(v)
        }
        TrsChannel::Rotation => {
            let q = transform.rotation;
            let mut v = [q.x, q.y, q.z, q.w];
            let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3]).sqrt();
            if !norm.is_finite() || norm == 0.0 {
                return None;
            }
            for x in &mut v {
                *x /= norm;
            }
            if let Some(reference) = reference {
                let dot = v[0] * reference[0]
                    + v[1] * reference[1]
                    + v[2] * reference[2]
                    + v[3] * reference[3];
                if dot < 0.0 {
                    for x in &mut v {
                        *x = -*x;
                    }
                }
            }
            Some(v)
        }
    }
}

/// Values + per-key Hermite tangents of one verified channel.
struct ExactTrack {
    /// Authored key times before the clip-time trim.
    times_raw: Vec<f64>,
    /// Clip times (trimmed) as stored in the Bevy curve; strictly increasing.
    times: Vec<f32>,
    /// Key values (`w = 0.0` for `Vec3` channels).
    values: Vec<[f64; 4]>,
    /// In-tangent slope of each key (first key's entry is unused by Bevy).
    slopes_in: Vec<[f64; 4]>,
    /// Out-tangent slope of each key (last key's entry is unused by Bevy).
    slopes_out: Vec<[f64; 4]>,
}

/// Sample the channel at `times_raw`, derive cubic-exact per-key slopes, and
/// verify every segment against ufbx at interior probe times.
///
/// Returns `None` on any guard failure (too-short segment, non-finite data,
/// collapsed `f32` times, or a probe whose Hermite differs from
/// `ufbx_evaluate_transform` beyond tolerance — the last one catches values
/// influenced by other animated properties, which the union of this channel's
/// own keys cannot represent).
fn build_exact_track(
    times_raw: Vec<f64>,
    shift: f64,
    channel: TrsChannel,
    anim: &ufbx::Anim,
    node: &ufbx::Node,
) -> Option<ExactTrack> {
    let count = times_raw.len();
    if count < 2 {
        return None;
    }
    for pair in times_raw.windows(2) {
        if pair[1] - pair[0] < EXACT_MIN_SEGMENT {
            return None;
        }
    }

    // Key values, with rotation continuity fixed key-over-key.
    let mut values = Vec::with_capacity(count);
    let mut reference: Option<[f64; 4]> = None;
    for &time in &times_raw {
        let value = eval_channel(anim, node, time, channel, reference.as_ref())?;
        reference = Some(value);
        values.push(value);
    }

    // Per-segment endpoint slopes: out-slope of key i from the forward stencil
    // in segment i, in-slope of key i+1 from the backward stencil. Rotation
    // differentiates all four quaternion components so the Vec4 Hermite
    // matches the true rotation path; Vec3 channels leave `w` at zero.
    let components = if channel == TrsChannel::Rotation {
        4
    } else {
        3
    };
    let mut slopes_in = vec![[0.0; 4]; count];
    let mut slopes_out = vec![[0.0; 4]; count];
    for segment in 0..count - 1 {
        let t0 = times_raw[segment];
        let t1 = times_raw[segment + 1];
        let h = (t1 - t0) / 4.0;

        let mut forward = [[0.0; 4]; 4];
        for (step, slot) in forward.iter_mut().enumerate() {
            *slot = eval_channel(
                anim,
                node,
                t0 + h * step as f64,
                channel,
                Some(&values[segment]),
            )?;
        }
        let mut backward = [[0.0; 4]; 4];
        for (step, slot) in backward.iter_mut().enumerate() {
            *slot = eval_channel(
                anim,
                node,
                t1 - h * step as f64,
                channel,
                Some(&values[segment + 1]),
            )?;
        }

        for component in 0..components {
            let mut f = [0.0; 4];
            let mut b = [0.0; 4];
            for step in 0..4 {
                f[step] = forward[step][component];
                b[step] = backward[step][component];
            }
            slopes_out[segment][component] = stencil_forward(&f, h);
            slopes_in[segment + 1][component] = stencil_backward(&b, h);
        }
    }

    // Verify each segment against ufbx at interior probe times.
    for segment in 0..count - 1 {
        let t0 = times_raw[segment];
        let t1 = times_raw[segment + 1];
        let dt = t1 - t0;
        let v0 = values[segment];
        let v1 = values[segment + 1];
        let m0 = slopes_out[segment];
        let m1 = slopes_in[segment + 1];
        for s in [0.2, 0.4, 0.6, 0.8] {
            let time = t0 + s * dt;
            let reference = if s < 0.5 { v0 } else { v1 };
            let truth = eval_channel(anim, node, time, channel, Some(&reference))?;
            if channel == TrsChannel::Rotation {
                let mut h = [0.0; 4];
                for component in 0..4 {
                    h[component] = hermite_f64(
                        v0[component],
                        m0[component],
                        v1[component],
                        m1[component],
                        s,
                        dt,
                    );
                }
                let norm = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2] + h[3] * h[3]).sqrt();
                if !norm.is_finite() || norm == 0.0 {
                    return None;
                }
                let dot =
                    (h[0] * truth[0] + h[1] * truth[1] + h[2] * truth[2] + h[3] * truth[3]) / norm;
                let angle = 2.0 * dot.abs().min(1.0).acos();
                let endpoint_dot = (v0[0] * v1[0] + v0[1] * v1[1] + v0[2] * v1[2] + v0[3] * v1[3])
                    .abs()
                    .min(1.0);
                let segment_angle = 2.0 * endpoint_dot.acos();
                if angle > 0.01 + 0.15 * segment_angle {
                    return None;
                }
            } else {
                for component in 0..3 {
                    let expected = hermite_f64(
                        v0[component],
                        m0[component],
                        v1[component],
                        m1[component],
                        s,
                        dt,
                    );
                    let scale =
                        1.0 + v0[component].abs() + v1[component].abs() + truth[component].abs();
                    let wiggle = (v1[component] - v0[component])
                        .abs()
                        .max((m0[component] * dt).abs())
                        .max((m1[component] * dt).abs());
                    let tolerance = 1e-3 * scale + 2e-2 * wiggle;
                    if (expected - truth[component]).abs() > tolerance {
                        return None;
                    }
                }
            }
        }
    }

    let times: Vec<f32> = times_raw.iter().map(|time| (time - shift) as f32).collect();
    for pair in times.windows(2) {
        if !pair[0].is_finite() || !(pair[1] > pair[0]) {
            return None;
        }
    }

    Some(ExactTrack {
        times_raw,
        times,
        values,
        slopes_in,
        slopes_out,
    })
}

/// Build the exact translation/scale curve for one channel, or fall back to
/// the baked track by returning `None`.
fn exact_vec3_curve(
    anim: &ufbx::Anim,
    node: &ufbx::Node,
    layer: &ufbx::AnimLayer,
    channel: TrsChannel,
    shift: f64,
) -> Option<CubicKeyframeCurve<Vec3>> {
    debug_assert!(channel != TrsChannel::Rotation);
    let times_raw = node_channel_key_times(layer, &node.element, channel.prop())?;
    let track = build_exact_track(times_raw, shift, channel, anim, node)?;
    let count = track.times_raw.len();
    let mut samples = Vec::with_capacity(count * 3);
    for key in 0..count {
        let zero = if key == 0 {
            [0.0; 4]
        } else {
            track.slopes_in[key]
        };
        let out = if key + 1 == count {
            [0.0; 4]
        } else {
            track.slopes_out[key]
        };
        samples.push(Vec3::new(zero[0] as f32, zero[1] as f32, zero[2] as f32));
        samples.push(Vec3::new(
            track.values[key][0] as f32,
            track.values[key][1] as f32,
            track.values[key][2] as f32,
        ));
        samples.push(Vec3::new(out[0] as f32, out[1] as f32, out[2] as f32));
    }
    CubicKeyframeCurve::new(track.times, samples).ok()
}

/// Build the exact rotation curve for one channel, or fall back to the baked
/// track by returning `None`.
fn exact_rotation_curve(
    anim: &ufbx::Anim,
    node: &ufbx::Node,
    layer: &ufbx::AnimLayer,
    shift: f64,
) -> Option<CubicRotationCurve> {
    let times_raw = node_channel_key_times(layer, &node.element, TrsChannel::Rotation.prop())?;
    let track = build_exact_track(times_raw, shift, TrsChannel::Rotation, anim, node)?;
    let count = track.times_raw.len();
    let mut samples = Vec::with_capacity(count * 3);
    for key in 0..count {
        let zero = if key == 0 {
            [0.0; 4]
        } else {
            track.slopes_in[key]
        };
        let out = if key + 1 == count {
            [0.0; 4]
        } else {
            track.slopes_out[key]
        };
        samples.push(Vec4::new(
            zero[0] as f32,
            zero[1] as f32,
            zero[2] as f32,
            zero[3] as f32,
        ));
        samples.push(Vec4::new(
            track.values[key][0] as f32,
            track.values[key][1] as f32,
            track.values[key][2] as f32,
            track.values[key][3] as f32,
        ));
        samples.push(Vec4::new(
            out[0] as f32,
            out[1] as f32,
            out[2] as f32,
            out[3] as f32,
        ));
    }
    CubicRotationCurve::new(track.times, samples).ok()
}

/// Wide cubic curve over morph weights: glTF CUBICSPLINE layout
/// `[in(N), value(N), out(N)]` per key with time-linear Hermite interpolation.
///
/// bevy_animation 0.19.1's `WideCubicKeyframeCurve` cannot carry morph curves:
/// its exact-key branch slices with the *full* `3N` width (`v[width..width*2]`
/// on a `3N` chunk — out of bounds, panics whenever a key time is sampled,
/// which is every animation start), and its between-key helper passes the end
/// value/end tangent arguments swapped (both verified by the probes in
/// `tests/animation_cubic.rs`). This type implements the semantics Bevy
/// documents for that curve, so `WeightsCurve` samples stay correct at and
/// between keys.
#[derive(Clone, Debug, Reflect)]
struct WideHermiteCurve {
    times: Vec<f32>,
    values: Vec<f32>,
}

impl WideHermiteCurve {
    /// Number of simultaneously interpolated channels.
    fn channels(&self) -> usize {
        self.values.len() / (self.times.len() * 3)
    }

    /// The value slot of key `index` (the middle third of its triple).
    fn value_slot(&self, index: usize) -> &[f32] {
        let width = self.channels();
        let base = index * width * 3 + width;
        &self.values[base..base + width]
    }
}

impl IterableCurve<f32> for WideHermiteCurve {
    fn domain(&self) -> Interval {
        Interval::new(self.times[0], self.times[self.times.len() - 1])
            .unwrap_or(Interval::EVERYWHERE)
    }

    fn sample_iter_unchecked(&self, t: f32) -> impl Iterator<Item = f32> {
        self.sample_vec(t).into_iter()
    }
}

impl WideHermiteCurve {
    /// Sample all channels at `t` (assumed within the domain).
    fn sample_vec(&self, t: f32) -> Vec<f32> {
        let last = self.times.len() - 1;
        // `!(t > first)` also covers NaN.
        if !(t > self.times[0]) {
            return self.value_slot(0).to_vec();
        }
        if t >= self.times[last] {
            return self.value_slot(last).to_vec();
        }
        let index = self.times.partition_point(|time| *time <= t) - 1;
        if self.times[index] == t {
            return self.value_slot(index).to_vec();
        }
        let t0 = self.times[index];
        let t1 = self.times[index + 1];
        let step = t1 - t0;
        let s = (t - t0) / step;
        let width = self.channels();
        let base0 = index * width * 3;
        let base1 = (index + 1) * width * 3;
        (0..width)
            .map(|channel| {
                let value_start = self.values[base0 + width + channel];
                let slope_out = self.values[base0 + width * 2 + channel];
                let value_end = self.values[base1 + width + channel];
                let slope_in = self.values[base1 + channel];
                let s2 = s * s;
                let s3 = s2 * s;
                let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
                let h10 = s3 - 2.0 * s2 + s;
                let h01 = -2.0 * s3 + 3.0 * s2;
                let h11 = s3 - s2;
                h00 * value_start + h10 * step * slope_out + h01 * value_end + h11 * step * slope_in
            })
            .collect()
    }
}

/// Build the exact cubic morph curve for a node's channels, or `None` to keep
/// the uniform-rate resampled fallback.
///
/// All channels share one curve, so every animated `DeformPercent` curve must
/// pass the same guards (constant extrapolation, only Linear/Cubic spans, at
/// least one Cubic span overall); otherwise the whole node falls back.
fn exact_morph_weights(
    layer: Option<&ufbx::AnimLayer>,
    anim: &ufbx::Anim,
    channels: &[&ufbx::BlendChannel],
    shift: f64,
) -> Option<WeightsCurve<WideHermiteCurve>> {
    let layer = layer?;
    if channels.is_empty() {
        return None;
    }

    // Union of authored key times across animated channels.
    let mut times: Vec<f64> = Vec::new();
    let mut saw_cubic = false;
    for channel in channels {
        let Some(anim_prop) = layer.find_anim_prop(&channel.element, "DeformPercent") else {
            continue; // static channel: constant weight, no key times
        };
        let (channel_times, channel_cubic) =
            union_cubic_key_times(anim_prop.anim_value.curves.iter().flatten().map(|c| &**c))?;
        saw_cubic |= channel_cubic;
        times.extend(channel_times);
    }
    let times_raw = finish_key_times(times, saw_cubic)?;

    let count = times_raw.len();
    let channel_count = channels.len();
    for pair in times_raw.windows(2) {
        if pair[1] - pair[0] < EXACT_MIN_SEGMENT {
            return None;
        }
    }

    // Key values for every channel (static channels evaluate to their constant).
    let mut values = vec![vec![0.0f64; channel_count]; count];
    for (index, &time) in times_raw.iter().enumerate() {
        for (slot, channel) in channels.iter().enumerate() {
            let weight = ufbx::evaluate_blend_weight(anim, channel, time);
            if !weight.is_finite() {
                return None;
            }
            values[index][slot] = weight;
        }
    }

    // Classify channels: animated ones (an anim prop with curves) get stencils
    // and probes; static ones are constant with zero tangents.
    let mut animated = vec![false; channel_count];
    for (slot, channel) in channels.iter().enumerate() {
        if let Some(anim_prop) = layer.find_anim_prop(&channel.element, "DeformPercent") {
            animated[slot] = anim_prop
                .anim_value
                .curves
                .iter()
                .flatten()
                .next()
                .is_some();
        }
    }

    let mut slopes_in = vec![vec![0.0f64; channel_count]; count];
    let mut slopes_out = vec![vec![0.0f64; channel_count]; count];
    for segment in 0..count - 1 {
        let t0 = times_raw[segment];
        let t1 = times_raw[segment + 1];
        let h = (t1 - t0) / 4.0;
        let mut forward = vec![vec![0.0f64; channel_count]; 4];
        let mut backward = vec![vec![0.0f64; channel_count]; 4];
        for slot in 0..channel_count {
            if !animated[slot] {
                continue;
            }
            for step in 0..4 {
                forward[step][slot] =
                    ufbx::evaluate_blend_weight(anim, channels[slot], t0 + h * step as f64);
                backward[step][slot] =
                    ufbx::evaluate_blend_weight(anim, channels[slot], t1 - h * step as f64);
                if !forward[step][slot].is_finite() || !backward[step][slot].is_finite() {
                    return None;
                }
            }
            let mut f = [0.0; 4];
            let mut b = [0.0; 4];
            for step in 0..4 {
                f[step] = forward[step][slot];
                b[step] = backward[step][slot];
            }
            slopes_out[segment][slot] = stencil_forward(&f, h);
            slopes_in[segment + 1][slot] = stencil_backward(&b, h);
        }

        // Verify animated channels against ufbx at interior probe times.
        for s in [0.2, 0.4, 0.6, 0.8] {
            let time = t0 + s * (t1 - t0);
            for slot in 0..channel_count {
                if !animated[slot] {
                    continue;
                }
                let expected = hermite_f64(
                    values[segment][slot],
                    slopes_out[segment][slot],
                    values[segment + 1][slot],
                    slopes_in[segment + 1][slot],
                    s,
                    t1 - t0,
                );
                let truth = ufbx::evaluate_blend_weight(anim, channels[slot], time);
                let scale = 1.0
                    + values[segment][slot].abs()
                    + values[segment + 1][slot].abs()
                    + truth.abs();
                let wiggle = (values[segment + 1][slot] - values[segment][slot])
                    .abs()
                    .max((slopes_out[segment][slot] * (t1 - t0)).abs())
                    .max((slopes_in[segment + 1][slot] * (t1 - t0)).abs());
                let tolerance = 1e-3 * scale + 2e-2 * wiggle;
                if !truth.is_finite() || (expected - truth).abs() > tolerance {
                    return None;
                }
            }
        }
    }

    let times_f32: Vec<f32> = times_raw.iter().map(|time| (time - shift) as f32).collect();
    for pair in times_f32.windows(2) {
        if !pair[0].is_finite() || !(pair[1] > pair[0]) {
            return None;
        }
    }

    let mut packed = Vec::with_capacity(count * channel_count * 3);
    for key in 0..count {
        for slot in 0..channel_count {
            let input = if key == 0 { 0.0 } else { slopes_in[key][slot] };
            packed.push(input as f32);
        }
        for slot in 0..channel_count {
            packed.push(values[key][slot] as f32);
        }
        for slot in 0..channel_count {
            let output = if key + 1 == count {
                0.0
            } else {
                slopes_out[key][slot]
            };
            packed.push(output as f32);
        }
    }

    Some(WeightsCurve(WideHermiteCurve {
        times: times_f32,
        values: packed,
    }))
}

/// Add a baked translation/scale track. A single key stays a `ConstantCurve`.
fn add_vec3_track(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    property: impl AnimatableProperty<Property = Vec3> + Clone,
    track: CleanTrack<Vec3>,
    node_path: &[Name],
    track_name: &str,
) {
    if track.dropped > 0 {
        warn_track(
            node_path,
            track_name,
            format_args!("{} key(s) were non-finite or collapsed", track.dropped),
        );
    }
    match track.sample_count() {
        0 => {}
        1 => {
            clip.add_curve_to_target(
                target_id,
                AnimatableCurve::new(
                    property,
                    ConstantCurve::new(Interval::EVERYWHERE, track.values[0]),
                ),
            );
        }
        _ if track.has_steps() => match BakedVec3Curve::new(track) {
            Some(curve) => {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
            None => warn_track(node_path, track_name, "not enough valid keys"),
        },
        _ => {
            let samples = track
                .times
                .iter()
                .copied()
                .zip(track.values.iter().copied());
            match UnevenSampleAutoCurve::new(samples) {
                Ok(curve) => {
                    clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
                }
                Err(err) => warn_track(node_path, track_name, err),
            }
        }
    }
}

/// Add a baked rotation track. A single key stays a `ConstantCurve`.
fn add_quat_track(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    property: impl AnimatableProperty<Property = Quat> + Clone,
    track: CleanTrack<Quat>,
    node_path: &[Name],
    track_name: &str,
) {
    if track.dropped > 0 {
        warn_track(
            node_path,
            track_name,
            format_args!("{} key(s) were non-finite or collapsed", track.dropped),
        );
    }
    match track.sample_count() {
        0 => {}
        1 => {
            clip.add_curve_to_target(
                target_id,
                AnimatableCurve::new(
                    property,
                    ConstantCurve::new(Interval::EVERYWHERE, track.values[0]),
                ),
            );
        }
        _ if track.has_steps() => match BakedQuatCurve::new(track) {
            Some(curve) => {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
            None => warn_track(node_path, track_name, "not enough valid keys"),
        },
        _ => {
            let samples = track
                .times
                .iter()
                .copied()
                .zip(track.values.iter().copied());
            match UnevenSampleAutoCurve::new(samples) {
                Ok(curve) => {
                    clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
                }
                Err(err) => warn_track(node_path, track_name, err),
            }
        }
    }
}

fn add_trs_curves(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    baked_node: &ufbx::BakedNode,
    node_path: &[Name],
    scene: &ufbx::Scene,
    anim: &ufbx::Anim,
    layer: Option<&ufbx::AnimLayer>,
    shift: f64,
) {
    // Exact cubic attempt per channel; any guard/probe failure falls back to
    // the baked track below, unchanged.
    let node = scene.nodes.get(baked_node.typed_id as usize);
    let exact_layer = layer.filter(|_| node.is_some_and(|n| exact_node_guard(n)));

    let translation_curve = exact_layer.and_then(|layer| {
        let node = node.expect("guarded above");
        exact_vec3_curve(anim, node, layer, TrsChannel::Translation, shift)
    });
    match translation_curve {
        Some(curve) => clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(animated_field!(Transform::translation), curve),
        ),
        None => add_vec3_track(
            clip,
            target_id,
            animated_field!(Transform::translation),
            clean_vec3_keys(&baked_node.translation_keys),
            node_path,
            "translation",
        ),
    }

    let rotation_curve = exact_layer.and_then(|layer| {
        let node = node.expect("guarded above");
        exact_rotation_curve(anim, node, layer, shift)
    });
    match rotation_curve {
        Some(curve) => clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(animated_field!(Transform::rotation), curve),
        ),
        None => add_quat_track(
            clip,
            target_id,
            animated_field!(Transform::rotation),
            clean_rotation_keys(&baked_node.rotation_keys),
            node_path,
            "rotation",
        ),
    }

    let scale_curve = exact_layer.and_then(|layer| {
        let node = node.expect("guarded above");
        exact_vec3_curve(anim, node, layer, TrsChannel::Scale, shift)
    });
    match scale_curve {
        Some(curve) => clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(animated_field!(Transform::scale), curve),
        ),
        None => add_vec3_track(
            clip,
            target_id,
            animated_field!(Transform::scale),
            clean_vec3_keys(&baked_node.scale_keys),
            node_path,
            "scale",
        ),
    }
}

/// Sample blend-channel weights into a [`WeightsCurve`] on each mesh node path.
///
/// Channels that are uniformly authored cubic take the exact wide-Hermite
/// path first ([`exact_morph_weights`]); everything else keeps the uniform
/// baked-rate resampling below.
fn add_morph_weight_curves(
    scene: &ufbx::Scene,
    anim: &ufbx::Anim,
    baked: &ufbx::BakedAnim,
    clip: &mut AnimationClip,
    bake_fps: f32,
    layer: Option<&ufbx::AnimLayer>,
    shift: f64,
) {
    let duration = baked.playback_duration as f32;
    if !duration.is_finite() || duration < 0.0 {
        warn!(
            "FBX animation: morph weight curves skipped: baked duration {} is not usable",
            baked.playback_duration
        );
        return;
    }

    let fps = bake_fps.max(1.0);

    for node in scene.nodes.as_ref().iter() {
        if node.is_root {
            continue;
        }
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh = mesh_ref.as_ref();
        if mesh.blend_deformers.is_empty() {
            continue;
        }

        let mut channels: Vec<&ufbx::BlendChannel> = Vec::new();
        for deformer in mesh.blend_deformers.as_ref().iter() {
            for channel in deformer.channels.as_ref().iter() {
                if channel.target_shape.is_none() {
                    continue;
                }
                if channels.len() >= MAX_MORPH_WEIGHTS {
                    break;
                }
                channels.push(channel);
            }
        }
        if channels.is_empty() {
            continue;
        }

        let path = animation_name_path(scene, node_typed_id(node));
        if path.is_empty() {
            continue;
        }
        let target_id = AnimationTargetId::from_names(path.iter());

        if let Some(curve) = exact_morph_weights(layer, anim, &channels, shift) {
            clip.add_curve_to_target(target_id, curve);
            continue;
        }

        let sample_count = ((duration * fps).ceil() as usize).clamp(1, 256);
        let mut times = Vec::with_capacity(sample_count + 1);
        let mut values = Vec::with_capacity((sample_count + 1) * channels.len());
        let mut non_finite = 0usize;

        for i in 0..=sample_count {
            let t = if sample_count == 0 {
                0.0
            } else {
                duration * (i as f32) / sample_count as f32
            };
            let ufbx_t = baked.playback_time_begin + f64::from(t);
            times.push(t);
            for channel in &channels {
                let weight = ufbx::evaluate_blend_weight(anim, channel, ufbx_t) as f32;
                if weight.is_finite() {
                    values.push(weight);
                } else {
                    non_finite += 1;
                    values.push(0.0);
                }
            }
        }

        if non_finite > 0 {
            warn_track(
                &path,
                "morph weights",
                format_args!("{non_finite} non-finite sample(s) were clamped to 0"),
            );
        }

        if times.len() == 1 {
            clip.add_curve_to_target(
                target_id,
                WeightsCurve(ConstantCurve::new(Interval::EVERYWHERE, values)),
            );
        } else {
            match WideLinearKeyframeCurve::new(times, values) {
                Ok(curve) => {
                    clip.add_curve_to_target(target_id, WeightsCurve(curve));
                }
                Err(err) => warn_track(&path, "morph weights", err),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_target() -> AnimationTargetId {
        AnimationTargetId::from_names(test_path().iter())
    }

    fn test_path() -> [Name; 1] {
        [Name::new("Node")]
    }

    fn vec3_key(time: f64, x: f32, y: f32, z: f32, flags: ufbx::BakedKeyFlags) -> ufbx::BakedVec3 {
        ufbx::BakedVec3 {
            time,
            value: ufbx::Vec3 {
                x: x as f64,
                y: y as f64,
                z: z as f64,
            },
            flags,
        }
    }

    fn quat_key(time: f64, q: Quat, flags: ufbx::BakedKeyFlags) -> ufbx::BakedQuat {
        ufbx::BakedQuat {
            time,
            value: ufbx::Quat {
                x: q.x as f64,
                y: q.y as f64,
                z: q.z as f64,
                w: q.w as f64,
            },
            flags,
        }
    }

    fn sample_vec3(keys: &[ufbx::BakedVec3], t: f32) -> Vec3 {
        let mut clip = AnimationClip::default();
        let target = test_target();
        add_vec3_track(
            &mut clip,
            target,
            animated_field!(Transform::translation),
            clean_vec3_keys(keys),
            &test_path(),
            "translation",
        );
        let curve = &clip.curves_for_target(target).expect("curve target")[0];
        *curve
            .0
            .sample_clamped(t)
            .downcast_ref::<Vec3>()
            .expect("Vec3 sample")
    }

    fn sample_quat(keys: &[ufbx::BakedQuat], t: f32) -> Quat {
        let mut clip = AnimationClip::default();
        let target = test_target();
        add_quat_track(
            &mut clip,
            target,
            animated_field!(Transform::rotation),
            clean_rotation_keys(keys),
            &test_path(),
            "rotation",
        );
        let curve = &clip.curves_for_target(target).expect("curve target")[0];
        *curve
            .0
            .sample_clamped(t)
            .downcast_ref::<Quat>()
            .expect("Quat sample")
    }

    /// Mirrors ufbx's own `maya_anim_linear` expectations: a `ConstantStandard` key at
    /// `t = 0.5` jumps to the spike value exactly at `0.5`, and the `ConstantNext` key drops
    /// back to the held value immediately after. The ±1 ms bake samples must stay flat.
    #[test]
    fn stepped_translation_holds_then_jumps_without_ramps() {
        let keys = [
            vec3_key(0.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(1.0 / 3.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(0.499, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::STEP_LEFT),
            vec3_key(
                0.5,
                0.0,
                0.0,
                2.0,
                ufbx::BakedKeyFlags::KEYFRAME | ufbx::BakedKeyFlags::STEP_KEY,
            ),
            vec3_key(0.501, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::STEP_RIGHT),
            vec3_key(14.0 / 24.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
        ];

        assert_eq!(sample_vec3(&keys, 0.25).z, 0.0);
        // Linear interpolation of the ±1 ms pair would give ~1.0 here.
        assert_eq!(sample_vec3(&keys, 0.4995).z, 0.0);
        assert_eq!(sample_vec3(&keys, 0.5).z, 2.0);
        assert_eq!(sample_vec3(&keys, 0.5005).z, 0.0);
        assert_eq!(sample_vec3(&keys, 0.52).z, 0.0);
    }

    /// `STEP_RIGHT` (`ConstantNext`) makes the whole segment before the key use its value.
    #[test]
    fn stepped_translation_step_right_uses_next_value_for_whole_segment() {
        let keys = [
            vec3_key(0.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(
                1.0,
                10.0,
                0.0,
                0.0,
                ufbx::BakedKeyFlags::KEYFRAME
                    | ufbx::BakedKeyFlags::STEP_RIGHT
                    | ufbx::BakedKeyFlags::STEP_KEY,
            ),
            vec3_key(2.0, 20.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
        ];

        assert_eq!(sample_vec3(&keys, 0.0).x, 0.0);
        assert_eq!(sample_vec3(&keys, 0.5).x, 10.0);
        assert_eq!(sample_vec3(&keys, 1.0).x, 10.0);
        // The segment after the step is plain linear again.
        assert_eq!(sample_vec3(&keys, 1.5).x, 15.0);
    }

    /// `STEP_KEY` alone is a marker for the key a step belongs to; it must not hold.
    #[test]
    fn step_key_flag_alone_stays_linear() {
        let keys = [
            vec3_key(0.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(
                1.0,
                10.0,
                0.0,
                0.0,
                ufbx::BakedKeyFlags::KEYFRAME | ufbx::BakedKeyFlags::STEP_KEY,
            ),
        ];

        assert_eq!(sample_vec3(&keys, 0.5).x, 5.0);
    }

    #[test]
    fn step_free_translation_stays_linear_and_single_key_is_constant() {
        let linear = [
            vec3_key(0.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(1.0, 10.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
        ];
        assert_eq!(sample_vec3(&linear, 0.25).x, 2.5);
        assert_eq!(sample_vec3(&linear, 0.5).x, 5.0);

        let constant = [vec3_key(0.5, 1.0, 2.0, 3.0, ufbx::BakedKeyFlags::KEYFRAME)];
        assert_eq!(sample_vec3(&constant, 0.0), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(sample_vec3(&constant, 100.0), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn nonfinite_times_and_values_are_dropped() {
        let keys = [
            vec3_key(f64::NAN, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(0.0, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(0.5, f32::NAN, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(1.0, 10.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
            vec3_key(f64::INFINITY, 0.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
        ];

        let track = clean_vec3_keys(&keys);
        assert_eq!(track.sample_count(), 2);
        assert_eq!(track.dropped, 3);

        let sample = sample_vec3(&keys, 0.5);
        assert!(sample.is_finite());
        assert_eq!(sample.x, 5.0);
    }

    /// `f64` step offsets collapse when stored as `f32` times; the first key wins so the
    /// track stays strictly increasing.
    #[test]
    fn duplicate_f32_times_keep_first_key() {
        let keys = [
            vec3_key(
                100_000.0 - 0.001,
                0.0,
                0.0,
                0.0,
                ufbx::BakedKeyFlags::STEP_LEFT,
            ),
            vec3_key(
                100_000.0,
                1.0,
                0.0,
                0.0,
                ufbx::BakedKeyFlags::KEYFRAME | ufbx::BakedKeyFlags::STEP_KEY,
            ),
            vec3_key(
                100_000.0 + 0.001,
                2.0,
                0.0,
                0.0,
                ufbx::BakedKeyFlags::STEP_RIGHT,
            ),
            vec3_key(100_001.0, 3.0, 0.0, 0.0, ufbx::BakedKeyFlags::KEYFRAME),
        ];

        let track = clean_vec3_keys(&keys);
        assert_eq!(track.sample_count(), 2);
        assert_eq!(track.dropped, 2);
        assert_eq!(track.values[0].x, 0.0);
        assert_eq!(track.values[1].x, 3.0);
    }

    #[test]
    fn antipodal_rotation_keys_are_made_continuous() {
        let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let keys = [
            quat_key(0.0, Quat::IDENTITY, ufbx::BakedKeyFlags::KEYFRAME),
            // Same rotation as `quarter`, but on the opposite hemisphere.
            quat_key(1.0, -quarter, ufbx::BakedKeyFlags::KEYFRAME),
            quat_key(
                2.0,
                Quat::from_rotation_z(std::f32::consts::PI),
                ufbx::BakedKeyFlags::KEYFRAME,
            ),
        ];

        let raw_next = Quat::from_xyzw(
            keys[1].value.x as f32,
            keys[1].value.y as f32,
            keys[1].value.z as f32,
            keys[1].value.w as f32,
        );
        assert!(
            raw_next.dot(Quat::IDENTITY) < 0.0,
            "test setup: the raw pair must start antipodal"
        );

        let track = clean_rotation_keys(&keys);
        assert!(track.values[1].dot(track.values[0]) > 0.0);
        assert!((track.values[1] - quarter).length() < 1e-6);
        assert!(track.values[2].dot(track.values[1]) >= 0.0);
    }

    #[test]
    fn stepped_rotation_keys_hold_until_the_key() {
        let start = Quat::IDENTITY;
        let end = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let keys = [
            quat_key(0.0, start, ufbx::BakedKeyFlags::KEYFRAME),
            quat_key(0.999, start, ufbx::BakedKeyFlags::STEP_LEFT),
            quat_key(
                1.0,
                end,
                ufbx::BakedKeyFlags::KEYFRAME | ufbx::BakedKeyFlags::STEP_KEY,
            ),
            quat_key(2.0, end, ufbx::BakedKeyFlags::KEYFRAME),
        ];

        // Linear interpolation would already be ~0.07 rad rotated here.
        assert!(sample_quat(&keys, 0.9995).dot(start) > 0.999_9);
        assert!(sample_quat(&keys, 1.0).dot(end).abs() > 0.999_9);
        assert!(sample_quat(&keys, 0.5).dot(start) > 0.999_9);
    }

    // ── Exact cubic path ────────────────────────────────────────────────────

    fn load_fixture(relative: &str) -> ufbx::SceneRoot {
        let path = format!("{}{relative}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        ufbx::load_memory(
            &bytes,
            ufbx::LoadOpts {
                generate_missing_normals: true,
                load_external_files: false,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("load {path}: {e:?}"))
    }

    fn first_stack(scene: &ufbx::Scene) -> &ufbx::AnimStack {
        &scene.anim_stacks.as_ref()[0]
    }

    /// Exact cubic path on `cube_anim.fbx`: all TRS channels are authored
    /// `Cubic|TangeantAuto`, so each channel must produce a Bevy cubic curve
    /// whose samples equal `ufbx_evaluate_transform` at the authored keys and
    /// follow it between them (auto tangents make the ufbx span a cubic
    /// polynomial in time, which time-linear Hermite reproduces exactly up to
    /// float rounding).
    #[test]
    fn cube_anim_builds_exact_cubic_curves() {
        let root = load_fixture("/assets/cube_anim.fbx");
        let scene: &ufbx::Scene = &root;
        let stack = first_stack(scene);
        let anim: &ufbx::Anim = &stack.anim;
        let layer = exact_anim_layer(anim).expect("cube_anim has one plain layer");
        let shift = clip_time_shift(anim);
        let node = ufbx::find_node(scene, "pCube1").expect("pCube1");
        assert!(exact_node_guard(node), "plain node must pass the guard");

        for channel in [TrsChannel::Translation, TrsChannel::Scale] {
            let times =
                node_channel_key_times(layer, &node.element, channel.prop()).expect("cubic keys");
            assert_eq!(times.len(), 2, "{:?}: two authored keys", channel);

            let curve = exact_vec3_curve(anim, node, layer, channel, shift)
                .unwrap_or_else(|| panic!("{:?}: exact curve", channel));
            let debug = format!("{curve:?}");
            assert!(debug.contains("CubicKeyframeCurve"), "wrong curve: {debug}");

            for &raw in &times {
                let transform = ufbx::evaluate_transform(anim, node, raw);
                let truth = match channel {
                    TrsChannel::Translation => transform.translation,
                    _ => transform.scale,
                };
                let expected = Vec3::new(truth.x as f32, truth.y as f32, truth.z as f32);
                let got = curve.sample_clamped((raw - shift) as f32);
                for axis in 0..3 {
                    let tol = 1e-4 * (1.0 + expected[axis].abs());
                    assert!(
                        (got[axis] - expected[axis]).abs() <= tol,
                        "{channel:?} key {raw}: got {got:?}, expected {expected:?}"
                    );
                }
            }

            for s in [0.25, 0.5, 0.75] {
                let raw = times[0] + s * (times[1] - times[0]);
                let transform = ufbx::evaluate_transform(anim, node, raw);
                let truth = match channel {
                    TrsChannel::Translation => transform.translation,
                    _ => transform.scale,
                };
                let expected = Vec3::new(truth.x as f32, truth.y as f32, truth.z as f32);
                let got = curve.sample_clamped((raw - shift) as f32);
                for axis in 0..3 {
                    let tol = 1e-3 * (1.0 + expected[axis].abs());
                    assert!(
                        (got[axis] - expected[axis]).abs() <= tol,
                        "{channel:?} interior s={s}: got {got:?}, expected {expected:?}"
                    );
                }
            }
        }

        let times = node_channel_key_times(layer, &node.element, TrsChannel::Rotation.prop())
            .expect("rotation keys");
        let curve = exact_rotation_curve(anim, node, layer, shift).expect("exact rotation");
        let debug = format!("{curve:?}");
        assert!(debug.contains("CubicRotationCurve"), "wrong curve: {debug}");

        for &raw in &times {
            let transform = ufbx::evaluate_transform(anim, node, raw);
            let expected = Quat::from_xyzw(
                transform.rotation.x as f32,
                transform.rotation.y as f32,
                transform.rotation.z as f32,
                transform.rotation.w as f32,
            );
            let got = curve.sample_clamped((raw - shift) as f32);
            assert!(
                got.dot(expected).abs() > 0.9999,
                "rotation key {raw}: got {got:?}, expected {expected:?}"
            );
        }
        // Interior rotation error budget: a two-point Vec4 Hermite over a
        // composed rotation is an approximation of ufbx's cubic-Euler path
        // (intrinsic to `CubicRotationCurve` semantics, same as bevy_gltf),
        // so the tolerance scales with the segment's rotation magnitude.
        let start = ufbx::evaluate_transform(anim, node, times[0]).rotation;
        let end = ufbx::evaluate_transform(anim, node, times[1]).rotation;
        let dot = (start.x * end.x + start.y * end.y + start.z * end.z + start.w * end.w)
            .abs()
            .min(1.0);
        let segment_angle = (2.0 * dot.acos()) as f32;
        for s in [0.25, 0.5, 0.75] {
            let raw = times[0] + s * (times[1] - times[0]);
            let transform = ufbx::evaluate_transform(anim, node, raw);
            let expected = Quat::from_xyzw(
                transform.rotation.x as f32,
                transform.rotation.y as f32,
                transform.rotation.z as f32,
                transform.rotation.w as f32,
            );
            let got = curve.sample_clamped((raw - shift) as f32);
            let angle = 2.0 * got.dot(expected).abs().min(1.0).acos();
            let tolerance = 0.02 + 0.15 * segment_angle;
            assert!(
                angle < tolerance,
                "rotation interior s={s}: angle error {angle} rad exceeds {tolerance} \
                 (segment angle {segment_angle} rad)"
            );
        }
    }

    /// Stepped keys (`ConstantPrev`/`ConstantNext` spans) and mixed authored
    /// interpolation must never take the exact path — they keep the baked
    /// step-aware / resampled fallbacks untouched.
    #[test]
    fn stepped_and_mixed_channels_fall_back() {
        let cases: [(&str, &str); 2] = [
            ("/assets/maya_anim_linear_7700_ascii.fbx", "stepped"),
            (
                "/../../libs/ufbx/data/maya_anim_interpolation_7700_ascii.fbx",
                "mixed cubic/linear/constant",
            ),
        ];
        for (relative, label) in cases {
            let root = load_fixture(relative);
            let scene: &ufbx::Scene = &root;
            let stack = first_stack(scene);
            let anim: &ufbx::Anim = &stack.anim;
            let layer = exact_anim_layer(anim).unwrap_or_else(|| panic!("{label}: layer"));
            let node = ufbx::find_node(scene, "pCube1").expect("pCube1");
            let times = node_channel_key_times(layer, &node.element, "Lcl Translation");
            assert!(
                times.is_none(),
                "{label}: translation channel must fall back, got {times:?}"
            );
        }
    }

    /// Multi-key all-cubic fixtures take the exact path and their curve samples
    /// match `ufbx_evaluate_transform` at every authored key.
    #[test]
    fn multi_key_cubic_fixtures_build_exact_curves() {
        for relative in [
            "/../../libs/ufbx/data/maya_transform_animation_7500_ascii.fbx",
            "/../../libs/ufbx/data/maya_tangent_spline_7700_ascii.fbx",
        ] {
            let root = load_fixture(relative);
            let scene: &ufbx::Scene = &root;
            let stack = first_stack(scene);
            let anim: &ufbx::Anim = &stack.anim;
            let layer = exact_anim_layer(anim).unwrap_or_else(|| panic!("{relative}: layer"));
            let shift = clip_time_shift(anim);

            let mut hits = 0usize;
            for node in scene.nodes.as_ref().iter() {
                for prop in ["Lcl Translation", "Lcl Rotation", "Lcl Scaling"] {
                    let Some(times) = node_channel_key_times(layer, &node.element, prop) else {
                        continue;
                    };
                    assert!(
                        times.len() >= 3,
                        "{relative} {prop}: expected at least three cubic keys, got {}",
                        times.len()
                    );
                    if prop == "Lcl Rotation" {
                        let curve = exact_rotation_curve(anim, node, layer, shift)
                            .unwrap_or_else(|| panic!("{relative} {prop}: exact curve"));
                        for &raw in &times {
                            let t = ufbx::evaluate_transform(anim, node, raw);
                            let expected = Quat::from_xyzw(
                                t.rotation.x as f32,
                                t.rotation.y as f32,
                                t.rotation.z as f32,
                                t.rotation.w as f32,
                            );
                            let got = curve.sample_clamped((raw - shift) as f32);
                            assert!(
                                got.dot(expected).abs() > 0.9999,
                                "{relative} {prop} key {raw}: got {got:?}, expected {expected:?}"
                            );
                        }
                    } else {
                        let channel = if prop == "Lcl Translation" {
                            TrsChannel::Translation
                        } else {
                            TrsChannel::Scale
                        };
                        let curve = exact_vec3_curve(anim, node, layer, channel, shift)
                            .unwrap_or_else(|| panic!("{relative} {prop}: exact curve"));
                        for &raw in &times {
                            let t = ufbx::evaluate_transform(anim, node, raw);
                            let truth = if prop == "Lcl Translation" {
                                t.translation
                            } else {
                                t.scale
                            };
                            let expected =
                                Vec3::new(truth.x as f32, truth.y as f32, truth.z as f32);
                            let got = curve.sample_clamped((raw - shift) as f32);
                            for axis in 0..3 {
                                let tol = 1e-4 * (1.0 + expected[axis].abs());
                                assert!(
                                    (got[axis] - expected[axis]).abs() <= tol,
                                    "{relative} {prop} key {raw}: got {got:?}, expected {expected:?}"
                                );
                            }
                        }
                    }
                    hits += 1;
                }
            }
            assert!(hits > 0, "{relative}: no exact cubic channel found");
        }
    }

    /// Morph weights: authored `DeformPercent` cubic keys become one wide
    /// Hermite `WeightsCurve` whose samples equal
    /// `ufbx_evaluate_blend_weight` at the keys and between them.
    #[test]
    fn morph_weights_build_exact_wide_hermite_curve() {
        let root = load_fixture("/assets/blend_shape_cube.fbx");
        let scene: &ufbx::Scene = &root;
        let stack = first_stack(scene);
        let anim: &ufbx::Anim = &stack.anim;
        let layer = exact_anim_layer(anim).expect("blend_shape_cube layer");
        let shift = clip_time_shift(anim);

        let mut found: Option<Vec<&ufbx::BlendChannel>> = None;
        for node in scene.nodes.as_ref().iter() {
            if node.is_root {
                continue;
            }
            let Some(mesh_ref) = node.mesh.as_ref() else {
                continue;
            };
            let mesh = mesh_ref.as_ref();
            if mesh.blend_deformers.is_empty() {
                continue;
            }
            let mut channels: Vec<&ufbx::BlendChannel> = Vec::new();
            for deformer in mesh.blend_deformers.as_ref().iter() {
                for channel in deformer.channels.as_ref().iter() {
                    if channel.target_shape.is_some() {
                        channels.push(channel);
                    }
                }
            }
            if !channels.is_empty() {
                found = Some(channels);
                break;
            }
        }
        let channels = found.expect("blend channels");

        let curve =
            exact_morph_weights(Some(layer), anim, &channels, shift).expect("exact morph curve");
        let debug = format!("{curve:?}");
        assert!(debug.contains("WideHermiteCurve"), "wrong curve: {debug}");

        let anim_prop = layer
            .find_anim_prop(&channels[0].element, "DeformPercent")
            .expect("DeformPercent anim prop");
        let (times, saw_cubic) =
            union_cubic_key_times(anim_prop.anim_value.curves.iter().flatten().map(|c| &**c))
                .expect("cubic DeformPercent keys");
        let times = finish_key_times(times, saw_cubic).expect("strict times");
        assert_eq!(times.len(), 3, "blend_shape_cube has three authored keys");

        for &raw in &times {
            let sampled: Vec<f32> = curve.0.sample_iter_clamped((raw - shift) as f32).collect();
            assert_eq!(sampled.len(), channels.len());
            for (slot, channel) in channels.iter().enumerate() {
                let truth = ufbx::evaluate_blend_weight(anim, channel, raw) as f32;
                assert!(
                    (sampled[slot] - truth).abs() <= 1e-4 * (1.0 + truth.abs()),
                    "morph key {raw} channel {slot}: got {}, expected {truth}",
                    sampled[slot]
                );
            }
        }
        for s in [0.3, 0.6] {
            let raw = times[0] + s * (times[1] - times[0]);
            let sampled: Vec<f32> = curve.0.sample_iter_clamped((raw - shift) as f32).collect();
            for (slot, channel) in channels.iter().enumerate() {
                let truth = ufbx::evaluate_blend_weight(anim, channel, raw) as f32;
                assert!(
                    (sampled[slot] - truth).abs() <= 2e-3 * (1.0 + truth.abs()),
                    "morph interior s={s} channel {slot}: got {}, expected {truth}",
                    sampled[slot]
                );
            }
        }
    }
}
