#!/usr/bin/env python3
"""Offline sanity only: does NOT replace cargo check, Rust unit tests or Metal smoke."""
from pathlib import Path
import csv, json, re
root=Path(__file__).resolve().parents[1]
sources=list((root/'src').glob('*.rs'))
required={'color.rs','lut.rs','raw.rs','gpu.rs','image_io.rs','main.rs','lib.rs'}
assert required <= {x.name for x in sources}
for path in sources:
    text=path.read_text()
    assert text.count('{')>=text.count('}'),str(path)
    assert 'todo!()' not in text and 'unimplemented!()' not in text, str(path)
shader=(root/'src/shader.wgsl').read_text()
assert 'texture_storage_2d<rgba8unorm' in shader
assert 'fn signed_log' in shader and 'fn tetrahedral' in shader
assert '@compute @workgroup_size' in shader
assert 'slint::Image::try_from' in (root/'src/main.rs').read_text()
assert 'lut_upload_count' in (root/'src/gpu.rs').read_text()
assert 'noirraw_signed_log_v1' in (root/'src/lut.rs').read_text()
assert 'request_redraw' in (root/'src/main.rs').read_text()
with (root/'examples/luts/LUT_Catalog.csv').open() as f:
    rows=list(csv.DictReader(f))
assert len(rows)==2
for row in rows:
    lut=root/'examples/luts'/row['relative_path']
    lines=lut.read_text().splitlines()
    n=int([x for x in lines if x.startswith('LUT_3D_SIZE')][0].split()[-1])
    data=[x for x in lines if x and x[0].isdigit()]
    assert len(data)==n**3, (lut,len(data),n)
    if 'linear' in row['input_color_space']:
        meta=json.loads(lut.with_suffix('.json').read_text())
        assert meta['input_space']=='linear_rec2020_signed_log'
        assert meta['shaper']=='noirraw_signed_log_v1'
print(f'SOURCE_SANITY_PASS: {len(sources)} Rust source files, WGSL GPU stage, 2 demo LUTs with valid 17^3 data')
