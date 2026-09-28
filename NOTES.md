# bevy_ufbx NOTES

Forked loader in `tir_refs/fbx` aiming at **Bevy glTF-class FBX parity** (Bevy 0.19 + ufbx 0.9).

## Parity bar

Match what `bevy_gltf` exposes at runtime, fed by ufbx — not 1:1 Autodesk FBX fidelity.

## Implemented

- Hierarchy `WorldAsset`, skins + IBM, bake_anim TRS + morph `WeightsCurve`
- Mesh attrs: color, tangent (or generated), UV0/UV1
- Shared mesh instances: same ufbx mesh `element_id` on multiple nodes reuses one `FbxMesh` / primitives
- Blend shapes → morph targets + `MorphWeights` / `MeshMorphWeights`
- Embedded textures + wrap modes; richer PBR factors (coat/transmission/IOR)
- NURBS → tessellate to `Mesh`
- Constraint / non-linear skinning: warn and continue
- Honest `axis_system` / `unit_scale` when `convert_coordinates: false`
- `FbxExtras` from **USER_DEFINED** custom props only (built-in / synthetic props skipped)
- `FbxMeshName` components
- `FbxMesh` container asset (glTF-style): `Mesh{N}` labels `FbxMesh`; Bevy meshes are `Mesh{N}/Primitive{P}`
- `FbxPrimitive` on `FbxMesh.primitives` (mesh + optional `StandardMaterial` + extras); `Fbx.primitive_meshes` flat Bevy [`Mesh`] list; `FbxNode.mesh` is `Option<Handle<FbxMesh>>`
- `FbxNode`: `children` + `skin` handles filled (two-pass label reservation, glTF-style)
- `FbxMaterial` container asset (glTF-style): `Material{N}` → `FbxMaterial` (`.material` is `StandardMaterial`); inverted twins stay `Material{N} (inverted)` as bare `StandardMaterial`
- World **neg-scale**: odd count of negative axes on **world** scale (not local `sx*sy*sz`) selects `Material{N} (inverted)` cull twin (skips double-sided)
- Texture samplers: FBX `wrap_u` / `wrap_v` → address modes; mag/min/mip from `FbxLoaderSettings::default_sampler` (or full replace via `override_sampler`) — ufbx 0.9 has no filter fields
- Blender PBR: `use_blender_pbr_material` when probe exporter is Blender binary/ascii
- Anim takes: `Animation{N}` labels + `Fbx.named_animations["Take 001"]` map (no second name label)

## Engine-impossible / bake-only

| FBX feature | Behavior |
|-------------|----------|
| Constraints / IK | Warning; bake TRS in DCC / bake_anim |
| Dual-quaternion skin | Warning; LBS approximation |
| Native NURBS | Tessellate only |
| Stereo / character / audio | Ignored |
| GPU instancing | Shared mesh handles only |

## LoadOpts (when `convert_coordinates`)

RH Y-up, metres. Default [`FbxSpaceConversion::Auto`]: probe exporter with `ignore_all_content`, then
- Blender binary/ascii → `AdjustTransforms` (undo export root ×100)
- otherwise (Maya/cm, Mixamo, unknown) → `ModifyGeometry` (bake units into verts; clean node scales)

Maya centimetre fixtures correctly become ~1 cm in world metres — examples may apply a **demo** root scale for readability.

## Fixtures

- `assets/cube_anim.fbx` — transform take
- `assets/rigged_triangle.fbx` — skin
- `assets/blend_shape_cube.fbx` — morphs
- `assets/nurbs_saddle.fbx` — NURBS tessellate

## Non-goals

- Forking / upstreaming into bevyengine
- Full T.I.R. game character wiring
