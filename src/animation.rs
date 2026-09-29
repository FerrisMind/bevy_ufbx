//! Bake FBX animation stacks into Bevy [`AnimationClip`]s.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::{animation_name_path, node_typed_id};
use crate::utils::convert_transform;
use bevy::animation::animation_curves::WeightsCurve;
use bevy::animation::gltf_curves::WideLinearKeyframeCurve;
use bevy::animation::prelude::{AnimatableCurve, AnimatableProperty};
use bevy::animation::{AnimationClip, AnimationTargetId, animated_field};
use bevy::asset::{Handle, LoadContext};
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

        for baked_node in baked.nodes.as_ref().iter() {
            animated_typed_ids.insert(baked_node.typed_id);
            let path = animation_name_path(scene, baked_node.typed_id);
            if path.is_empty() {
                continue;
            }
            let target_id = AnimationTargetId::from_names(path.iter());
            add_trs_curves(&mut clip, target_id, baked_node, &path);
        }

        add_morph_weight_curves(scene, &stack.anim, &baked, &mut clip, bake_fps);

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
) {
    add_vec3_track(
        clip,
        target_id,
        animated_field!(Transform::translation),
        clean_vec3_keys(&baked_node.translation_keys),
        node_path,
        "translation",
    );
    add_quat_track(
        clip,
        target_id,
        animated_field!(Transform::rotation),
        clean_rotation_keys(&baked_node.rotation_keys),
        node_path,
        "rotation",
    );
    add_vec3_track(
        clip,
        target_id,
        animated_field!(Transform::scale),
        clean_vec3_keys(&baked_node.scale_keys),
        node_path,
        "scale",
    );
}

/// Sample blend-channel weights into a [`WeightsCurve`] on each mesh node path.
fn add_morph_weight_curves(
    scene: &ufbx::Scene,
    anim: &ufbx::Anim,
    baked: &ufbx::BakedAnim,
    clip: &mut AnimationClip,
    bake_fps: f32,
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
}
