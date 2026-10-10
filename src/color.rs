//! Explicit color conventions: camera input -> linear Rec.2020 D65 -> encoded sRGB delivery.
//! Linear scene values may exceed 1.0. Clamping occurs only at SDR output.

pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

pub fn srgb_linear_to_rec2020(v: [f32; 3]) -> [f32; 3] {
    [
        0.6274040*v[0] + 0.3292820*v[1] + 0.0433136*v[2],
        0.0690970*v[0] + 0.9195400*v[1] + 0.0113612*v[2],
        0.0163916*v[0] + 0.0880132*v[1] + 0.8955950*v[2],
    ]
}

pub fn rec2020_to_srgb_linear(v: [f32; 3]) -> [f32; 3] {
    [
        1.6604910*v[0] - 0.5876411*v[1] - 0.0728499*v[2],
       -0.1245505*v[0] + 1.1328999*v[1] - 0.0083494*v[2],
       -0.0181508*v[0] - 0.1005789*v[1] + 1.1187297*v[2],
    ]
}

/// Signed logarithmic scene-linear encoding. LUTs MUST be generated with
/// inverse-sampled values using this exact definition; a relabel is insufficient.
pub fn signed_log_encode(x: f32) -> f32 {
    if x < 0.0 { 0.2 * (1.0 - (1.0 + (-x).min(2.0) * 8.0).log2() / 17.0_f32.log2()) }
    else { 0.2 + 0.8 * (1.0 + x.min(64.0)*8.0).log2() / 513.0_f32.log2() }
}

pub fn signed_log_decode(u: f32) -> f32 {
    if u < 0.2 { -((2.0_f32).powf((0.2-u)/0.2*17.0_f32.log2())-1.0)/8.0 }
    else { ((2.0_f32).powf((u-0.2)/0.8*513.0_f32.log2())-1.0)/8.0 }
}

pub fn rec2020_to_srgb_encoded(v: [f32; 3]) -> [f32; 3] {
    rec2020_to_srgb_linear(v).map(|c| srgb_encode(c.max(0.0)).clamp(0.0,1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn signed_log_roundtrips() { for v in [-2.,-0.5,0.,0.01,0.18,1.,5.,16.,64.] {let b=signed_log_decode(signed_log_encode(v)); assert!((b-v).abs()<1e-4, "{v}: {b}"); } }
    #[test] fn srgb_roundtrips() { for v in [0.,0.001,0.1,0.5,0.9,1.] {assert!((srgb_encode(srgb_decode(v))-v).abs()<0.00001)} }
    #[test] fn color_neutral_roundtrips() { let v=[0.18,0.33,0.71]; let l=srgb_linear_to_rec2020(v);let s=rec2020_to_srgb_linear(l);for k in 0..3 {assert!((s[k]-v[k]).abs()<0.00003);} }
}
