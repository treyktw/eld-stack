//! Confidence scoring from cross-source agreement.

/// Score how well a secondary distance estimate agrees with the primary.
///
/// Returns 0.0–1.0. With no usable secondary data, returns a moderate 0.7
/// (the primary stands on its own). Otherwise the ratio of the smaller to the
/// larger maps linearly onto 0.5–1.0: perfect agreement → 1.0, total
/// disagreement → 0.5.
pub fn agreement(primary_km: f32, secondary_km: f32) -> f32 {
    if secondary_km <= 0.1 {
        return 0.7;
    }
    let (lo, hi) = if primary_km < secondary_km {
        (primary_km, secondary_km)
    } else {
        (secondary_km, primary_km)
    };
    if hi <= 0.0 {
        return 0.0;
    }
    let ratio = (lo / hi).clamp(0.0, 1.0);
    (0.5 + 0.5 * ratio).clamp(0.0, 1.0)
}
