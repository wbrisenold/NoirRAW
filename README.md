# NoirRAW — new native Rust photo editor (v0.1 prototype)

**This is a new project, not a SpektraFilm Studio port.** It does not clone or link the SpektraFilm application. It uses a deliberately small Rust image core, Slint for a genuinely native window (not HTML, Electron, Tauri or a webview), and wgpu 30 on Apple's Metal backend.

## What is actually implemented

- Native Slint photo editor with **Open Photo, LUT Folder, LUT picker, Exposure, Contrast, LUT Strength and Export Full Size**.
- Read JPEG, PNG, TIFF, BMP, WebP using `image`. Read supported camera RAW using `rawloader`, sensor black/white normalization, camera WB, pattern-aware neighborhood demosaic, camera matrix and D50→D65→Rec.2020 conversion. **Prototype-quality RAW reconstruction**, not yet validated against camera manufacturer colors and not an advanced denoising/demosaic system.
- Scene-linear Rec.2020 float working image. 1080px bound on editing proxy.
- One wgpu/Metal compute shader for preview and full-original-resolution **PNG** export. On editing, the preview is owned by a GPU texture and given directly to Slint; no per-slider GPU→CPU image readback.
- Existing 3D `.cube` import from one linked folder, recursive search, catalog-based human-friendly names using `LUT_Catalog.csv`, refresh button, remembered folder in `~/Library/Application Support/NoirRAW/preferences.json` (or platform equivalent).
- GPU-resident `.cube` LUTs, tetrahedral interpolation, adjustable strength; no LUT rebaking because imported LUTs are already baked. Selector reports GPU upload counters so repeat editing can be verified.
- Two explicit LUT contracts: existing sRGB(encoded)→sRGB and signed-log scene-linear Rec.2020→sRGB. The linear contract must be declared via a same-basename JSON sidecar containing `"shaper": "noirraw_signed_log_v1"` or `"spektrafilm_signed_log_v1"` and `"stage":"full_film_print"`.
- Full-film+print LUT bypasses the native film simulation because this fresh app has no native stock+paper simulator. Files are never relabeled to another color space silently.
- Includes Rust unit tests, a direct Metal smoke test, and GitHub Actions Intel macOS source build. License: GPL-3.0-or-later.

## Build on your Intel Mac

Requires macOS 12+, Rust toolchain and Apple's macOS SDK/Command Line Tools (full Xcode **not necessarily** required). Intel-compatible wgpu/Metal GPU required. Download ZIP, extract, and run:

```sh
cd NoirRAW-Rust
cargo generate-lockfile
cargo run --release
```

Or double-click `BUILD_ON_MAC.command` to run tests, run the Metal smoke test and build a native `.app` bundle. You can also upload the repository contents to a NEW GitHub repo: `.github/workflows/intel-macos.yml` will attempt to build an `x86_64` Mac app on a `macos-15-intel` runner and upload it as an Actions artifact.

If Gatekeeper blocks an unsigned local build, review the source and use Finder's **Open** shortcut/context menu or locally codesign/notarize it. The script applies only an ad-hoc signature; it does not claim Apple notarization.

## LUT catalog

Use **LUT Folder**, select the top-level output directory of your SpektraFilm LUT Matrix exporter, and optionally put a `LUT_Catalog.csv` in that directory. Supported catalog column aliases include:

- File: `relative_path`, `lut_path`, `path`, `filename`, `lut_file`, `lut_filename`.
- Display: `display_name`, `name`, `label`, `title`.
- Negative stock: `film_stock`, `film_profile`, `film`, `negative`.
- Paper: `print_profile`, `print_paper`, `paper`, `print`.
- Color: `input_color_space`, `input_space`, `input_colorspace`.

The absolute path stays the file's identity; label comes from the catalog, otherwise filename. An ambiguous basename does not get another file's metadata. Invalid or unsupported color-space contracts are skipped until corrected; **there is no dangerous guessing of linear Rec.2020**.

## Correct LUT contract

Ordinary pre-existing `.cube` without metadata is **assumed sRGB(encoded)→sRGB**. Any input from scene-linear Rec.2020 must be intentionally baked from the inverse grid **before** being tagged as such.

For a linear LUT, create a same-basename `.json` next to `.cube`:

```json
{
  "input_space": "linear_rec2020_signed_log",
  "output_space": "sRGB",
  "shaper": "noirraw_signed_log_v1",
  "stage": "full_film_print"
}
```

The signed-log mapping used by the shader is precisely specified in `src/color.rs` (Python reference: `scripts/validate_shaper.py`). A standard 3D `.cube` grid normally represents 0..1; scene-linear Rec.2020 values outside that interval are first transformed using the shaper, *then* indexed into the LUT. Simply labeling an sRGB LUT as Rec.2020 produces incorrect color and is not supported.

**Not yet supported:** `.3dl`, Hald PNG LUTs, multi-stage 1/2/3/4-LUT OCIO bundles, 1D shaper + 3D chains, ACES or ProPhoto workflow, film/paper stage mixing, advanced highlight recovery, tethering/culling/masks, baked in-camera DCP ICC profiles, 16-bit TIFF export, 10-bit HDR output, or GPU demosaicing. Unsupported LUT constructs are rejected rather than silently skipped. The `examples/luts/` folder contains two neutral 17³ LUTs (one sRGB and one correctly shaper-sampled Rec.2020) for testing; these do **not** recreate any film stock.

## Quality and smoke testing

```sh
cargo test --all-targets
cargo run --release -- --smoke-gpu
python3 scripts/validate_shaper.py
```

The GPU smoke uses 16x12 scene-linear test pixels and a synthetic identity `.cube` to check output values, scene-linear shaper behavior, and that changing exposure **doesn't reupload or rebake a LUT**. On macOS, it requires the actual backend to be **Metal**. This is not a substitute for testing a real CR3/ARW/RAF photo with calibrated reference output.

## Design/performance invariants

1. RAW decompression/demosaic happens on a background worker when a photo is opened; the decoded full-resolution source is retained for export. Editing adjusts uniform parameters, not the source decoder.
2. A GPU LUT is uploaded on selection; per-slider changes reuse the existing source+LUT textures and overwrite the same output texture.
3. The new app does not have a competing "settled native film engine" that can replace an active LUT frame.
4. Full-resolution PNG export uses `src/shader.wgsl` exactly like live preview. Output encoding is currently 8-bit SDR, so it is **not** archival 16-bit master quality.
5. The app does not modify existing SpektraFilm Studio repositories or LUT source files.

## Known engineering constraints

- This source was authored in a Linux environment without `cargo` or Apple SDK and **has not been compiled on an Intel Mac yet**. The included macOS Actions job and executable smoke test are the build gate, not a claimed success.
- RAW color conversion and basic demosaic are placeholders for a fuller RAW processor. Input profile handling and per-camera validation are necessary before photography production use.
- Full-resolution export currently reads decoded image data on the GPU and maps one final 8-bit frame to the CPU to write PNG. That GPU readback is intentional, unlike interactive edits.
- RAW/photo decode runs off-thread with latest-wins publication; export still blocks the UI while the GPU readback and file save complete. Export jobs need a separate worker/queue in the next performance pass.
- Original image could use gigabytes of RAM for very high resolution RAWs; GPU texture limits and memory budgets must be tested on Intel GPUs.

## External components & licenses

- Slint — https://github.com/slint-ui/slint (GPL-3.0 option)
- wgpu — https://github.com/gfx-rs/wgpu
- rawloader — https://github.com/pedrocr/rawloader
- image — https://github.com/image-rs/image
- rfd — https://github.com/PolyMeilex/rfd

No SpektraFilm profiles, LUTs, existing app branding or UI source have been copied into this project. User-imported LUTs may carry their own attribution/share-alike requirements.
