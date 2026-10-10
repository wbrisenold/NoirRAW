# NoirRAW Troubleshooting & Lessons Learned

This document captures all issues encountered during development and their fixes to prevent recurrence.

---

## 1. CR3 RAW Format Support

### Issue
Original `rawloader` crate doesn't support Canon CR3 format (MPEG-4 based container).

### Fix
Switched to `zenraw` with `rawler` feature flag:
```toml
zenraw = { version = "0.2", features = ["rawler"] }
```
This enables CR3, X-Trans (Fujifilm), and JXL-compressed DNG support.

---

## 2. Slint UI Syntax Errors

### 2.1 `border: 1px solid` Not Supported
**Error**: `Parse error: expected ';'`

**Fix**: Use separate properties:
```slint
border-width: 1px;
border-color: DesignTokens.divider;
```

### 2.2 `padding` on Non-Layout Elements
**Warning**: `padding-top only has effect on layout elements`

**Fix**: Only use `padding-*` on layout elements (`VerticalLayout`, `HorizontalLayout`, `GridLayout`, etc.). Use explicit `x`, `y`, `width`, `height` on `Rectangle`/`Text`/`Image`.

### 2.3 `columns` / `column-spacing` / `row-spacing` in GridLayout
**Error**: `Unknown property column-spacing in GridLayout`

**Fix**: Use:
```slint
GridLayout {
    spacing-horizontal: DesignTokens.cell_padding;
    spacing-vertical: DesignTokens.cell_padding;
}
```

### 2.4 `transparent` / `white` Color Constants
**Error**: `Unknown unqualified identifier 'transparent'`

**Fix**: Use `Colors.transparent` and `Colors.white` from std-widgets.

### 2.5 `.to-string()` on int
**Error**: `Cannot access the field 'to-string' of int`

**Fix**: Use string interpolation:
```slint
Text { text: "{root.thumbnails.length} images"; }
```

### 2.6 Global Design Tokens Declaration
**Error**: `Syntax error: expected Identifier` with `export global DesignTokens := {`

**Fix**: Use proper global struct syntax with explicit types:
```slint
export global DesignTokens {
    out property <brush> chrome: #2d2d2d;
    out property <length> panel_w: 270px;
    // ...
}
```

### 2.7 Button Styling Properties
**Error**: `Unknown property background in Button`

**Fix**: std-widgets Button uses theme styling. Use `primary: true` for accent buttons. Custom styling requires inheriting Button and overriding internal Rectangle.

### 2.8 `Keys` Element Not Found
**Error**: `Unknown element 'Keys'`

**Fix**: Use `FocusScope` with `key-pressed` callback:
```slint
FocusScope {
    key-pressed(key) => {
        root.key-pressed(key.text);
        reject
    }
}
```

### 2.9 Point Struct Construction
**Error**: `Syntax error: expected ')'` with `Point { x: event.x, y: event.y }`

**Fix**: Use `self.mouse_x` / `self.mouse_y` from TouchArea:
```slint
root.right_clicked({ x: self.mouse_x, y: self.mouse_y });
```

### 2.10 LineEdit Background/Color Properties
**Error**: `Unknown property background in LineEdit`

**Fix**: std-widgets LineEdit doesn't expose background/color. Use default styling or custom component.

### 2.11 Slider Track/Thumb Colors
**Error**: `Unknown property track-color in Slider`

**Fix**: std-widgets Slider uses theme. Custom colors require inheriting Slider.

### 2.12 ComboBox Background/Border
**Error**: `Unknown property background in ComboBox`

**Fix**: std-widgets ComboBox uses theme. Custom styling requires custom component.

---

## 3. Rust Closure Parsing Issues

### Issue: Mismatched Closing Delimiter in `app.on_select_lut`

**Error**: `mismatched closing delimiter: ')'` - Rust compiler couldn't parse the closure due to complex nested borrows.

**Root Cause**: The closure captured `state` and `app_weak` with complex borrowing patterns that confused the parser.

**Fix**: Extract to a standalone function:
```rust
fn handle_select_lut(
    app: &MainWindow,
    state: Rc<RefCell<State>>,
    value: SharedString,
) {
    // ... logic here
}

// Then call:
app.on_select_lut(move |value| {
    handle_select_lut(&app, state.clone(), value);
});
```

### Pattern to Avoid
Don't nest complex `let` bindings and `.clone()` calls inside closure parameter lists:
```rust
// BAD - causes parser confusion
app.on_select_lut(move |value| {
    let value_str = value.to_string();
    let app_weak = app_weak.clone();
    let state = state.clone();
    // ...
});
```

### Better Pattern
```rust
// Extract to function
fn handle_select_lut(app: &MainWindow, state: Rc<RefCell<State>>, value: SharedString) { ... }

// Simple closure
app.on_select_lut(move |value| {
    handle_select_lut(&app, state.clone(), value);
});
```

---

## 4. Slint Live Preview Setup

### Enable Live Preview
```bash
# Add to Cargo.toml
slint = { version = "=1.18.1", features = ["unstable-wgpu-30", "renderer-femtovg-wgpu", "live-preview"] }

# Run with:
SLINT_LIVE_PREVIEW=1 cargo run --features slint/live-preview
```

### Key Files for Live Preview
- `.slint` files auto-reload on save
- Rust logic stays connected during reload
- Changes to Rust code still require rebuild

---

## 5. ORT (ONNX Runtime) Version

### Issue: yanked versions
`ort = "1.16"` → all 1.16.x versions yanked.

**Fix**: Use latest RC:
```toml
ort = { version = "=2.0.0-rc.13", optional = true }
```

---

## 6. Cargo Feature Configuration

### Issue: Feature definitions in wrong section
**Error**: `invalid type: map, expected a sequence` for `ort = { version = "1.16", optional = true }` under `[features]`

**Fix**: Move optional deps to `[dependencies]`, only list feature names in `[features]`:
```toml
[dependencies]
ort = { version = "=2.0.0-rc.13", optional = true }
ndarray = { version = "0.16", optional = true }
tokenizers = { version = "0.23", optional = true }

[features]
default = []
face-recognition = ["ort", "ndarray", "tokenizers"]
```

---

## 7. Missing Crates

### `exif` crate
Only version `0.0.1` exists on crates.io (abandoned).

**Fix**: Use `kamadak-exif` instead:
```toml
kamadak-exif = "0.6"
```

### `image-hash` crate
Doesn't exist.

**Fix**: Use `img_hash`:
```toml
img_hash = "3.0"
```

---

## 8. Database Schema

### Primary Key Type
Use `INTEGER PRIMARY KEY AUTOINCREMENT` for SQLite. `i64` for row IDs.

### Indices
Create indices on frequently queried columns:
```sql
CREATE INDEX IF NOT EXISTS idx_images_sha256 ON images(sha256);
CREATE INDEX IF NOT EXISTS idx_images_rating ON images(rating);
CREATE INDEX IF NOT EXISTS idx_faces_person_id ON faces(person_id);
```

---

## 9. Thumbnail Generation

### Pattern: Two-Layer Fade-In
```rust
// Store multiple layers with opacity
struct ImageLayer { id: String, url: String, opacity: f32 }

// On new thumbnail: prepend with opacity 0
// Animate: opacity 0 → 1
// Cleanup: remove finished layers
```

### Thread Safety
```rust
// ThumbnailCache with Arc<Mutex<HashMap>>
struct ThumbnailCache {
    cache: Arc<Mutex<HashMap<String, Image>>>,
}
```

---

## 10. Keyboard Shortcuts Mapping

| Key | Action |
|-----|--------|
| `←` `→` | Navigate |
| `1`-`5` | Rating (0 clears) |
| `P` / `X` | Pick / Reject |
| `6`-`9` | Color labels (Red/Yellow/Green/Blue) |
| `Backspace` | Purple label |
| `Space` | Select |
| `G` | Expand to exposure bracket |
| `Enter` / `F` | Loupe view |
| `C` | Compare view |
| `I` | Metadata inspector |
| `K` | Keywords |
| `F` | Fullscreen |

---

## 11. XMP Sidecar Format

### Cullimingo Compatible
```xml
<x:xmpmeta>
  <rdf:RDF>
    <rdf:Description>
      <xmp:Rating>1</xmp:Rating>           <!-- 1-5 stars, -1 reject -->
      <xmp:Label>Green</xmp:Label>         <!-- Red/Yellow/Green/Blue/Purple -->
      <cullimingo:PickReject>1</cullimingo:PickReject>  <!-- 1=pick, -1=reject -->
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
```

---

## 12. Face Detection Pipeline

### Dependencies (feature-gated)
```toml
[features]
face-recognition = ["ort", "ndarray", "tokenizers"]
```

### Model Download
```bash
# Auto-downloads on first run (~100MB)
# SCRFD: scrfd_500m_bnkps.onnx (face detection)
# ArcFace: w600k_r50.onnx (512-dim embeddings)
```

### Clustering
```rust
// DBSCAN-style fallback (configurable via env)
SEEN_FACE_CLUSTER_EPSILON=0.55
SEEN_FACE_HDBSCAN_MIN_CLUSTER_SIZE=3
SEEN_FACE_HDBSCAN_MIN_SAMPLES=2
```

---

## 13. Slint Version Compatibility

### Minimum Version
Slint ≥ 1.17 required for:
- `live-preview` feature
- `slint-viewer` tool
- Modern widget APIs

### Version Pinning
```toml
slint = { version = "=1.18.1", features = [...] }
slint-build = "=1.18.1"
```

---

## 14. Git Workflow

### Before Commit
```bash
# Check formatting
cargo fmt --check

# Run clippy
cargo clippy -- -D warnings

# Build with live preview
SLINT_LIVE_PREVIEW=1 cargo build --features slint/live-preview
```

---

## 15. Known Unfixed Issues

1. **Select LUT callback delimiter parsing** - Works when extracted to function, fails inline
2. **Padding warnings on non-layout elements** - Cosmetic, doesn't affect functionality
2. **Face recognition needs GPU** - CPU fallback works but slower
3. **Thumbnail cache memory** - No eviction policy implemented

---

## Quick Reference: Slint Property Types

| Type | Example |
|------|---------|
| `<brush>` | `#ff0000`, `Colors.red` |
| `<length>` | `10px`, `1rem` |
| `<color>` | `#ff0000` (alias for brush) |
| `<int>` | `42` |
| `<float>` | `3.14` |
| `<string>` | `"hello"` |
| `<bool>` | `true` |

---

## 16. Useful Commands

```bash
# Build with live preview
SLINT_LIVE_PREVIEW=1 cargo run --features slint/live-preview

# Check Slint file syntax
slint-viewer --check ui/app.slint

# Screenshot for review
slint-viewer --screenshot ui/app.slint

# Auto-reload during development
slint-viewer --auto-reload ui/app.slint

# Run only face recognition (feature-gated)
cargo run --features face-recognition
```

---

*Last updated: 2026-10-10*
*Keep this updated as new issues are discovered*