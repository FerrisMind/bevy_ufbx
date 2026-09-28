# bevy_ufbx NOTES

Forked loader in `tir_refs/fbx` aiming at **Bevy glTF-class FBX parity** (Bevy 0.19 + ufbx 0.9).

## Parity bar

Match what `bevy_gltf` exposes at runtime, fed by ufbx — not 1:1 Autodesk FBX fidelity.

## Implemented

- Hierarchy `WorldAsset`, skins + IBM, bake_anim TRS + morph `WeightsCurve`
- Mesh attrs: color, tangent (or generated), UV0/UV1
- Blend shapes → morph targets + `MorphWeights` / `MeshMorphWeights`
- Embedded textures + wrap modes; richer PBR factors (coat/transmission/IOR)
- NURBS → tessellate to `Mesh`
- Constraint / non-linear skinning: warn and continue
- Honest `axis_system` / `unit_scale` when `convert_coordinates: false`
- `FbxExtras`, `FbxMeshName` components

## Engine-impossible / bake-only

| FBX feature | Behavior |
|-------------|----------|
| Constraints / IK | Warning; bake TRS in DCC / bake_anim |
| Dual-quaternion skin | Warning; LBS approximation |
| Native NURBS | Tessellate only |
| Stereo / character / audio | Ignored |
| GPU instancing | Shared mesh handles only |

## LoadOpts (when `convert_coordinates`)

RH Y-up, metres, `AdjustTransforms`, `HelperNodes`, `InheritModeHandling::Compensate`.

**Units caveat:** Maya/ufbx fixtures often have `unit_meters = 0.01` (cm). With `AdjustTransforms`, mesh vertex AABB stays in file units (e.g. ±0.5) while the **node local scale** becomes `0.01`, so the world-space cube is **~1 cm**. Examples that need a metre-sized subject must root-scale by `~100` (see `examples/morph_fbx.rs`).

## Fixtures

- `assets/cube_anim.fbx` — transform take
- `assets/rigged_triangle.fbx` — skin
- `assets/blend_shape_cube.fbx` — morphs
- `assets/nurbs_saddle.fbx` — NURBS tessellate

## Non-goals

- Forking / upstreaming into bevyengine
- Full T.I.R. game character wiring
