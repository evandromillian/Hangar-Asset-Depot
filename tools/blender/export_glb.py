"""Hangar headless conversion: open/import any Blender-supported file and export a GLB.

Usage (invoked by hangar-pipeline):
    blender --background --factory-startup --python-exit-code 3 \
        --python export_glb.py -- <input> <output.glb>

Supports .blend (opened directly) and whatever importers the running Blender
ships: fbx, obj, gltf/glb, dae, stl, ply, 3ds (if the add-on is enabled), usd.
"""

import os
import sys

import bpy


def fail(msg):
    print(f"[hangar] ERROR: {msg}", file=sys.stderr)
    sys.exit(3)


def args():
    argv = sys.argv
    if "--" not in argv:
        fail("expected: -- <input> <output.glb>")
    rest = argv[argv.index("--") + 1:]
    if len(rest) < 2:
        fail("expected: -- <input> <output.glb>")
    return os.path.abspath(rest[0]), os.path.abspath(rest[1])


def clear_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def try_ops(candidates, filepath):
    """Call the first operator that exists in this Blender version."""
    last_error = None
    for path, kwargs in candidates:
        mod, name = path.split(".")
        op = getattr(getattr(bpy.ops, mod), name, None)
        if op is None:
            continue
        try:
            op(filepath=filepath, **kwargs)
            return
        except Exception as exc:  # noqa: BLE001 - report the last failure
            last_error = exc
    fail(f"no importer could read {filepath}: {last_error}")


IMPORTERS = {
    ".fbx": [("import_scene.fbx", {})],
    ".obj": [("wm.obj_import", {}), ("import_scene.obj", {})],
    ".glb": [("import_scene.gltf", {})],
    ".gltf": [("import_scene.gltf", {})],
    ".dae": [("wm.collada_import", {})],
    ".stl": [("wm.stl_import", {}), ("import_mesh.stl", {})],
    ".ply": [("wm.ply_import", {}), ("import_mesh.ply", {})],
    ".3ds": [("import_scene.max3ds", {}), ("import_scene.autodesk_3ds", {})],
    ".usd": [("wm.usd_import", {})],
    ".usdc": [("wm.usd_import", {})],
    ".usdz": [("wm.usd_import", {})],
    ".abc": [("wm.alembic_import", {})],
}


def main():
    src, dst = args()
    ext = os.path.splitext(src)[1].lower()
    if ext == ".blend":
        bpy.ops.wm.open_mainfile(filepath=src)
    else:
        clear_scene()
        if ext not in IMPORTERS:
            fail(f"unsupported extension {ext}")
        try_ops(IMPORTERS[ext], src)

    # Make paths inside the file resolvable and pack images so textures end up in the GLB.
    try:
        bpy.ops.file.make_paths_absolute()
    except Exception:  # noqa: BLE001
        pass
    try:
        bpy.ops.file.pack_all()
    except Exception as exc:  # noqa: BLE001 - missing textures are not fatal
        print(f"[hangar] warning: pack_all failed: {exc}", file=sys.stderr)

    os.makedirs(os.path.dirname(dst), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=dst,
        export_format="GLB",
        export_apply=True,  # apply modifiers (mirror, subdivision...)
        export_yup=True,
        export_texcoords=True,
        export_normals=True,
        export_materials="EXPORT",
        export_animations=False,
        export_cameras=False,
        export_lights=False,
    )
    if not os.path.exists(dst):
        fail("export produced no file")
    print(f"[hangar] exported {dst}")


if __name__ == "__main__":
    main()
