#!/usr/bin/env python3
"""GUI launcher for bevy_ufbx cargo examples.

Run from anywhere:
  python run_examples_gui.py

Working directory is always this crate root (where Cargo.toml lives).
Requires: Python 3.10+, Rust/cargo on PATH. Uses stdlib tkinter only.
"""

from __future__ import annotations

import os
import shlex
import subprocess
import sys
import threading
import tkinter as tk
from dataclasses import dataclass
from pathlib import Path
from tkinter import filedialog, messagebox, scrolledtext, ttk

CRATE_ROOT = Path(__file__).resolve().parent


@dataclass(frozen=True)
class Example:
    name: str
    title: str
    blurb: str
    needs_arg: bool = False
    default_arg: str = ""


EXAMPLES: list[Example] = [
    Example("showcase_fbx", "Showcase", "morph | anim | nurbs | skin | multi-mat side-by-side"),
    Example("morph_fbx", "Morph / blend shapes", "WeightsCurve on blend_shape_cube"),
    Example("animated_mesh_fbx", "Animated mesh (TRS)", "baked take via AnimationGraph"),
    Example("skinned_mesh_fbx", "Skinned mesh", "LBS sausage / rigged_triangle"),
    Example("nurbs_fbx", "NURBS tessellate", "nurbs_saddle → triangle Mesh"),
    Example("static_mesh_fbx", "Static mesh", "Suzanne / cube hierarchy + materials"),
    Example("multimaterial_fbx", "Multi-material", "material-split color regions"),
    Example("nested_meshes_fbx", "Nested meshes", "cube / cone / ico / plane hierarchy"),
    Example("textures_fbx", "Embedded textures", "internal textures + wrap"),
    Example("lights_cameras_fbx", "Lights & cameras", "FBX light + activated Camera3d"),
    Example("vertex_color_fbx", "Vertex color", "ZBrush ATTRIBUTE_COLOR"),
    Example("neg_scale_fbx", "Negative scale cull", "mirrored mesh → inverted material"),
    Example("load_fbx", "Load FBX (CLI)", "generic viewer", needs_arg=True, default_arg="cube.fbx"),
    Example("dump_fbx", "Dump FBX (headless)", "print asset summary", needs_arg=True, default_arg="cube.fbx"),
]


class ExampleLauncher(tk.Tk):
    def __init__(self) -> None:
        super().__init__()
        self.title("bevy_ufbx examples")
        self.geometry("780x560")
        self.minsize(640, 420)

        self._proc: subprocess.Popen[str] | None = None
        self._reader: threading.Thread | None = None

        self._build_ui()
        self._select_first()
        self.protocol("WM_DELETE_WINDOW", self._on_close)

    def _build_ui(self) -> None:
        root = ttk.Frame(self, padding=10)
        root.pack(fill=tk.BOTH, expand=True)

        hdr = ttk.Label(
            root,
            text=f"Crate: {CRATE_ROOT}",
            font=("Segoe UI", 9),
        )
        hdr.pack(anchor=tk.W)

        body = ttk.Panedwindow(root, orient=tk.HORIZONTAL)
        body.pack(fill=tk.BOTH, expand=True, pady=(8, 8))

        left = ttk.Frame(body)
        right = ttk.Frame(body)
        body.add(left, weight=1)
        body.add(right, weight=2)

        ttk.Label(left, text="Examples", font=("Segoe UI", 10, "bold")).pack(anchor=tk.W)
        self.listbox = tk.Listbox(left, exportselection=False, font=("Consolas", 10), height=18)
        self.listbox.pack(fill=tk.BOTH, expand=True, pady=(4, 0))
        for ex in EXAMPLES:
            self.listbox.insert(tk.END, f"{ex.name}")
        self.listbox.bind("<<ListboxSelect>>", self._on_select)
        self.listbox.bind("<Double-Button-1>", lambda _e: self._run())

        ttk.Label(right, text="Details", font=("Segoe UI", 10, "bold")).pack(anchor=tk.W)
        self.detail = ttk.Label(right, text="", wraplength=420, justify=tk.LEFT)
        self.detail.pack(anchor=tk.W, pady=(4, 8), fill=tk.X)

        arg_row = ttk.Frame(right)
        arg_row.pack(fill=tk.X, pady=(0, 6))
        ttk.Label(arg_row, text="Extra args:").pack(side=tk.LEFT)
        self.arg_var = tk.StringVar()
        self.arg_entry = ttk.Entry(arg_row, textvariable=self.arg_var)
        self.arg_entry.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=(6, 4))
        ttk.Button(arg_row, text="Browse…", command=self._browse_fbx).pack(side=tk.LEFT)

        opts = ttk.Frame(right)
        opts.pack(fill=tk.X, pady=(0, 8))
        self.release_var = tk.BooleanVar(value=False)
        ttk.Checkbutton(opts, text="--release", variable=self.release_var).pack(side=tk.LEFT)
        self.quiet_var = tk.BooleanVar(value=False)
        ttk.Checkbutton(opts, text="quiet cargo (-q)", variable=self.quiet_var).pack(
            side=tk.LEFT, padx=(12, 0)
        )

        btns = ttk.Frame(right)
        btns.pack(fill=tk.X, pady=(0, 8))
        self.run_btn = ttk.Button(btns, text="Run", command=self._run)
        self.run_btn.pack(side=tk.LEFT)
        self.stop_btn = ttk.Button(btns, text="Stop", command=self._stop, state=tk.DISABLED)
        self.stop_btn.pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(btns, text="Clear log", command=self._clear_log).pack(side=tk.LEFT, padx=(8, 0))

        ttk.Label(right, text="Log", font=("Segoe UI", 10, "bold")).pack(anchor=tk.W)
        self.log = scrolledtext.ScrolledText(
            right, height=16, font=("Consolas", 9), state=tk.DISABLED, wrap=tk.WORD
        )
        self.log.pack(fill=tk.BOTH, expand=True, pady=(4, 0))

        self.status = ttk.Label(root, text="Ready", relief=tk.SUNKEN, anchor=tk.W)
        self.status.pack(fill=tk.X, pady=(4, 0))

    def _select_first(self) -> None:
        self.listbox.selection_set(0)
        self._on_select()

    def _selected(self) -> Example | None:
        sel = self.listbox.curselection()
        if not sel:
            return None
        return EXAMPLES[sel[0]]

    def _on_select(self, _event: object | None = None) -> None:
        ex = self._selected()
        if not ex:
            return
        self.detail.config(text=f"{ex.title}\n\n{ex.blurb}\n\ncargo run --example {ex.name}")
        if ex.needs_arg and not self.arg_var.get().strip():
            self.arg_var.set(ex.default_arg)

    def _browse_fbx(self) -> None:
        path = filedialog.askopenfilename(
            title="Pick FBX under assets/ (or absolute)",
            initialdir=CRATE_ROOT / "assets",
            filetypes=[("FBX", "*.fbx"), ("All", "*.*")],
        )
        if not path:
            return
        p = Path(path)
        try:
            rel = p.resolve().relative_to((CRATE_ROOT / "assets").resolve())
            self.arg_var.set(rel.as_posix())
        except ValueError:
            self.arg_var.set(str(p))

    def _append_log(self, text: str) -> None:
        self.log.configure(state=tk.NORMAL)
        self.log.insert(tk.END, text)
        self.log.see(tk.END)
        self.log.configure(state=tk.DISABLED)

    def _clear_log(self) -> None:
        self.log.configure(state=tk.NORMAL)
        self.log.delete("1.0", tk.END)
        self.log.configure(state=tk.DISABLED)

    def _set_running(self, running: bool) -> None:
        self.run_btn.configure(state=tk.DISABLED if running else tk.NORMAL)
        self.stop_btn.configure(state=tk.NORMAL if running else tk.DISABLED)
        self.status.configure(text="Running…" if running else "Ready")

    def _run(self) -> None:
        if self._proc is not None:
            messagebox.showinfo("Busy", "An example is already running. Stop it first.")
            return
        ex = self._selected()
        if not ex:
            return
        if not (CRATE_ROOT / "Cargo.toml").is_file():
            messagebox.showerror("Error", f"Cargo.toml not found in\n{CRATE_ROOT}")
            return

        cmd = ["cargo", "run"]
        if self.quiet_var.get():
            cmd.append("-q")
        if self.release_var.get():
            cmd.append("--release")
        cmd.extend(["--example", ex.name])

        extra = self.arg_var.get().strip()
        if extra:
            cmd.append("--")
            cmd.extend(shlex.split(extra, posix=os.name != "nt"))

        self._clear_log()
        self._append_log(f"$ {' '.join(cmd)}\n  cwd: {CRATE_ROOT}\n\n")
        self._set_running(True)

        creationflags = 0
        if os.name == "nt":
            creationflags = subprocess.CREATE_NEW_PROCESS_GROUP  # type: ignore[attr-defined]

        try:
            self._proc = subprocess.Popen(
                cmd,
                cwd=CRATE_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                bufsize=1,
                creationflags=creationflags,
            )
        except FileNotFoundError:
            self._set_running(False)
            self._proc = None
            messagebox.showerror("Error", "cargo not found on PATH")
            return
        except OSError as e:
            self._set_running(False)
            self._proc = None
            messagebox.showerror("Error", str(e))
            return

        self._reader = threading.Thread(target=self._pump_output, daemon=True)
        self._reader.start()

    def _pump_output(self) -> None:
        assert self._proc is not None
        proc = self._proc
        try:
            assert proc.stdout is not None
            for line in proc.stdout:
                self.after(0, self._append_log, line)
            code = proc.wait()
            self.after(0, self._append_log, f"\n[exit {code}]\n")
        finally:
            self.after(0, self._finished)

    def _finished(self) -> None:
        self._proc = None
        self._reader = None
        self._set_running(False)

    def _stop(self) -> None:
        proc = self._proc
        if proc is None:
            return
        self._append_log("\n[stopping…]\n")
        try:
            if os.name == "nt":
                subprocess.run(
                    ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                    capture_output=True,
                    check=False,
                )
            else:
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
        except OSError as e:
            self._append_log(f"[stop error: {e}]\n")

    def _on_close(self) -> None:
        if self._proc is not None:
            self._stop()
        self.destroy()


def main() -> None:
    if not (CRATE_ROOT / "Cargo.toml").is_file():
        print(f"Cargo.toml missing next to script: {CRATE_ROOT}", file=sys.stderr)
        sys.exit(1)
    app = ExampleLauncher()
    app.mainloop()


if __name__ == "__main__":
    main()
