# TekkCAM CLI

The `tekkcam` binary runs project and CAM workflows without opening the GUI. It is offline: it does not enumerate, connect to, or send commands to machine controllers. The desktop application is built as `tekkcam-gui`.

## Build and install

Build both binaries from the repository root:

```sh
cargo build --release --bins
```

The CLI executable is `target/release/tekkcam`.

## Typical workflow

```sh
tekkcam import --project-out board.tkc board-top.gbr board.drl
tekkcam setup set --project board.tkc --stock-size 100,80,20 --safe-z 5 --machine-min 0,0,-5 --machine-max 300,200,100
tekkcam operations add --project board.tkc --kind isolation --name "Top isolation"
tekkcam operations add --project board.tkc --kind drill --name "Drill hits"
tekkcam validate --project board.tkc
tekkcam generate --project board.tkc --output board.nc
```

Supported operation kinds are `isolation`, `pocket`, `drill`, `helical-bore`, `contour`, `model-profile-2d`, `roughing-3d`, `finish-raster-3d`, `finish-contour-3d`, and `finish-waterline-3d`. `operations add --tool INDEX` selects a zero-based entry from the project's tool library. `setup show` displays persisted setup. `setup set` accepts stock/work origins, stock size, axis, safe Z, clearance, machining parameters, tolerances, material, work offset, and machine min/max coordinates. Tool definitions and detailed per-operation parameters remain available in the `.tkc` JSON project.

Import accepts Gerber, Excellon, DXF, SVG, and STEP files. `inspect` reports imported object counts and STEP units/bounds. `operations list` displays order and generated move counts. `operations set --index 0 --depth 1.2 --stepdown 0.4 --margin 0.1 --tool 0` updates operation parameters. Cut options include `--ramp-angle DEG` (0-45; contour and profile entry ramps, 0 plunges), `--peck MM` and `--chip-break MM` (drilling; chip break must be smaller than the peck), and `--stepover-ratio R` (pocket stepover as a fraction of tool diameter, default 0.8). `operations reorder --order 1,0` requires every existing zero-based operation index exactly once and saves the project.

`validate` checks setup ranges, tool definitions, STEP mesh data, and configured travel limits when generated paths are present. It reports import notes, tessellation and sampled-check limitations, the selected post, millimeter units, work offset, and whether travel limits are configured. `generate` regenerates each operation, runs G-code validation, writes the requested `.nc` file (default: the project path with an `.nc` extension), and saves generated toolpaths back into the project. It never moves a machine.

## Automatic operation planning

For an imported STEP model, `plan` chooses the operations the part needs, generates their toolpaths and saves them in the project:

```sh
tekkcam import --project-out part.tkc part.step
tekkcam plan --project part.tkc --fit-stock
tekkcam generate --project part.tkc --output part.nc
```

The planner roughs the stock when there is material to remove, mills every round hole it finds with a **helical bore** (nothing is drilled, for low-torque, high-RPM spindles; the cutter size is assumed from the hole diameter, not taken from the tool library, and holes under 1.3 mm are reported and skipped), then finishes steep walls (waterline) and shallow areas (raster) with a ball-nose cutter where stock remains. Options: `--fit-stock` fits the stock around the model first, `--roughing-tool MM` and `--finishing-tool MM` set the largest cutter sizes tried, `--steep-angle DEG` sets the steep/shallow split (default 40°), and `--dynamic-stock on|off` switches the in-process stock check (default on; stored in the project setup as `dynamic_stock_check`). Re-running `plan` replaces the operations of the previous plan and keeps hand-made ones. The model must be Z-up and inside the stock. Planned cutters are limited by the stock margin around the part, so a larger margin allows a larger roughing cutter.

The dynamic stock check replays each planned path on a height-field of the stock left by the operations before it. It reports tool-holder contact with uncut stock and rapid moves into material, skips holes already cleared, and plans finishing only where stock is left. It is a 3-axis, top-down approximation and is meant as a warning-grade check, like the other STEP checks.

## GPU acceleration

`plan` and `generate` use the GPU for the heavy checks (cutter access, gouge and holder tests, stock simulation) when an adapter is available, and fall back to the CPU otherwise. The results are the same either way: the GPU decides what it can prove and the exact mesh query on the CPU answers the rest. `--gpu off` forces the CPU, `--gpu on` fails if no GPU can be opened, and `--gpu auto` (the default) uses a GPU if there is one. In the desktop application the switch is under Settings → Performance.

## Automation output and exit codes

Add `--json` to a command for stable JSON output:

```json
{
  "schema_version": 1,
  "command": "validate",
  "result": {
    "valid": true,
    "errors": [],
    "warnings": []
  }
}
```

Consumers should check `schema_version` before interpreting `result`. Human-readable output is the default. Errors go to stderr with exit code 2 for command syntax/usage, 3 for project/setup/CAM validation, and 4 for input/output or serialization failures. Successful commands return 0.

Project files use JSON and the `.tkc` extension in the GUI and CLI. Existing `.tekkCAM` and `.json` project files remain supported. JSON report compatibility is versioned separately with `schema_version`.
