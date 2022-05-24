// SPDX-License-Identifier: MPL-2.0-only

/// utility function which maps a value [0, 1] -> [0, 1] using the smootherstep function
pub fn smootherstep(t: f32) -> f32 {
    (6.0 * t.powi(5) - 15.0 * t.powi(4) + 10.0 * t.powi(3)).clamp(0.0, 1.0)
}
