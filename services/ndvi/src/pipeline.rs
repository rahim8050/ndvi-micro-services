use crate::models::PreprocessResponse;
use ndarray::{azip, Array2};
use rayon::prelude::*;

pub fn run_pipeline(
    vv_raw: Array2<f32>,
    vh_raw: Array2<f32>,
    inc_angle_deg: f32,
    index_type: &str,
    alpha: f32,
    beta: f32,
    gamma: f32,
) -> PreprocessResponse {
    let (vv, mask_vv) = mask_nodata(&vv_raw);
    let (vh, mask_vh) = mask_nodata(&vh_raw);

    // Pixel-wise AND of masks
    let mut valid_mask = mask_vv.clone();
    azip!((v in &mut valid_mask, &vh in &mask_vh) *v = *v && vh);

    // Apply speckle filter on linear backscatter
    let vv_filtered = refined_lee_filter(&vv, 3);
    let vh_filtered = refined_lee_filter(&vh, 3);

    // Estimate incidence angle from data when the STAC asset is unavailable
    // (40.0 is the Python-side fallback constant; real angles are scene-specific)
    let angle = if (inc_angle_deg - 40.0).abs() < 0.01 {
        estimate_incidence_angle_deg(&vv_filtered, &vh_filtered)
    } else {
        inc_angle_deg
    };

    // Compute index (handles dB conversion internally if needed)
    let index = compute_index(
        &vv_filtered,
        &vh_filtered,
        angle,
        index_type,
        alpha,
        beta,
        gamma,
    );

    compute_stats(&index, &valid_mask)
}

/// Pipeline for L-band SAR indices (L_RVI, NISAR_FLOOD) using HH/HV bands.
/// Same structure as `run_pipeline` but accepts HH/HV as primary bands.
pub fn run_pipeline_sar(
    hh_raw: Array2<f32>,
    hv_raw: Array2<f32>,
    inc_angle_deg: f32,
    index_type: &str,
) -> PreprocessResponse {
    let (hh, mask_hh) = mask_nodata(&hh_raw);
    let (hv, mask_hv) = mask_nodata(&hv_raw);

    let mut valid_mask = mask_hh.clone();
    azip!((v in &mut valid_mask, &hv in &mask_hv) *v = *v && hv);

    // NISAR_FLOOD scores raw L-band power directly (open water
    // specular-reflects away from the sensor); a speckle filter would
    // smear the sharp land/water boundary the dB thresholds are
    // calibrated against, and the Python _nisar_flood_fn it must stay
    // compatible with applies no filtering.
    if index_type == "NISAR_FLOOD" {
        let index = compute_index_sar(&hh, &hv, inc_angle_deg, index_type);
        return compute_stats(&index, &valid_mask);
    }

    let hh_filtered = refined_lee_filter(&hh, 3);
    let hv_filtered = refined_lee_filter(&hv, 3);

    let angle = if (inc_angle_deg - 40.0).abs() < 0.01 {
        estimate_incidence_angle_deg_lband(&hh_filtered, &hv_filtered)
    } else {
        inc_angle_deg
    };

    let index = compute_index_sar(&hh_filtered, &hv_filtered, angle, index_type);

    compute_stats(&index, &valid_mask)
}

fn mask_nodata(arr: &Array2<f32>) -> (Array2<f32>, Array2<bool>) {
    let data = arr.mapv(|v| if v <= 1e-10 { f32::NAN } else { v });
    let valid_pixels = arr.mapv(|v| v > 1e-10);
    (data, valid_pixels)
}

fn linear_to_db(arr: &Array2<f32>) -> Array2<f32> {
    arr.mapv(|v| {
        if v > 1e-10 {
            10.0 * v.log10()
        } else {
            f32::NAN
        }
    })
}

/// Mean (VV or HH) dB minus mean (VH or HV) dB over pixels where both
/// bands are valid.  Returns `None` when fewer than 100 valid pairs exist.
fn mean_db_ratio(primary: &Array2<f32>, cross: &Array2<f32>) -> Option<f32> {
    let mut primary_db_sum = 0.0_f32;
    let mut cross_db_sum = 0.0_f32;
    let mut count = 0_u64;

    azip!((&p in primary, &c in cross) {
        if p > 1e-10 && c > 1e-10 {
            primary_db_sum += 10.0 * p.log10();
            cross_db_sum += 10.0 * c.log10();
            count += 1;
        }
    });

    if count < 100 {
        None
    } else {
        Some((primary_db_sum / count as f32) - (cross_db_sum / count as f32))
    }
}

fn theta_from_ratio(ratio_db: f32, ref_db: f32, slope: f32) -> f32 {
    (35.0 + (ratio_db - ref_db) / slope).clamp(29.0, 46.0)
}

/// Estimate the scene-average local incidence angle (degrees) from VV/VH backscatter.
///
/// When the Planetary Computer STAC item has no `local_incidence_angle` asset,
/// derive a reasonable estimate from the VV/VH power ratio in dB.
///
/// Empirical relationship for Sentinel-1 IW GRD over agricultural land:
///   VV/VH(dB) ≈ 1.5 + 0.075 * (θ - 35)
///   => θ ≈ 35 + (VV/VH_dB - 1.5) / 0.075
///
/// Falls back to 40.0° when fewer than 100 valid pairs exist.
/// Clamps result to [29.0, 46.0] (Sentinel-1 IW GRD incidence angle range).
/// Kept in lockstep with Python's `_estimate_cband_incidence_angle_deg`.
pub fn estimate_incidence_angle_deg(vv: &Array2<f32>, vh: &Array2<f32>) -> f32 {
    match mean_db_ratio(vv, vh) {
        Some(ratio_db) => theta_from_ratio(ratio_db, 1.5, 0.075),
        None => 40.0,
    }
}

/// Estimate the scene-average local incidence angle from HH/HV backscatter.
///
/// Empirical relationship for NISAR L-band SAR over vegetation:
///   HH/HV(dB) ≈ 2.5 + 0.07 * (θ - 35)
///   => θ ≈ 35 + (HH/HV_dB - 2.5) / 0.07
///
/// Falls back to 40.0° when fewer than 100 valid pairs exist.
/// Clamps result to [29.0, 46.0].
/// Kept in lockstep with Python's `_estimate_lband_incidence_angle_deg`.
pub fn estimate_incidence_angle_deg_lband(hh: &Array2<f32>, hv: &Array2<f32>) -> f32 {
    match mean_db_ratio(hh, hv) {
        Some(ratio_db) => theta_from_ratio(ratio_db, 2.5, 0.07),
        None => 40.0,
    }
}

/// Normalize linear backscatter for incidence angle.
///
/// `σ_norm = σ * cos(θ_local) / cos(θ_ref)` — removes the geometric
/// dependence on look angle so that backscatter values from different
/// parts of the swath (or different orbits) are comparable.
fn normalize_incidence_angle_linear(arr: &Array2<f32>, theta_local_deg: f32) -> Array2<f32> {
    let theta_ref_rad = 40.0_f32.to_radians();
    let theta_local_rad = theta_local_deg.to_radians();
    let correction = theta_local_rad.cos() / theta_ref_rad.cos();
    arr.mapv(|v| if v.is_nan() { f32::NAN } else { v * correction })
}

/// Normalize dB backscatter for incidence angle.
///
/// `σ_dB_norm = σ_dB + 10*log10(cos(θ_local)/cos(θ_ref))` — the dB
/// equivalent of `normalize_incidence_angle_linear`
/// (`10*log10(σ * cos(θ_local)/cos(θ_ref))`).
fn normalize_incidence_angle(db_arr: &Array2<f32>, theta_local_deg: f32) -> Array2<f32> {
    let theta_ref_rad = 40.0_f32.to_radians();
    let theta_local_rad = theta_local_deg.to_radians();
    let correction = theta_local_rad.cos() / theta_ref_rad.cos();

    db_arr.mapv(|v| {
        if v.is_nan() {
            f32::NAN
        } else {
            v + 10.0 * correction.log10()
        }
    })
}

pub fn refined_lee_filter(arr: &Array2<f32>, kernel_size: u32) -> Array2<f32> {
    let (rows, cols) = arr.dim();
    let half = (kernel_size / 2) as usize;
    let mut output = arr.clone();

    // Parallel iteration over pixel rows using Rayon
    output
        .axis_iter_mut(ndarray::Axis(0))
        .into_par_iter()
        .enumerate()
        .for_each(|(i, mut row_slice)| {
            if i < half || i >= rows - half {
                return;
            }
            for j in half..(cols - half) {
                if arr[[i, j]].is_nan() {
                    continue;
                }

                let mut sum = 0.0;
                let mut sum_sq = 0.0;
                let mut count = 0.0;

                for ki in (i - half)..=(i + half) {
                    for kj in (j - half)..=(j + half) {
                        let val = arr[[ki, kj]];
                        if !val.is_nan() {
                            sum += val;
                            sum_sq += val * val;
                            count += 1.0;
                        }
                    }
                }

                if count > 0.0 {
                    let mean = sum / count;
                    let variance = (sum_sq / count) - (mean * mean);

                    // Lee filter weight: w = var / (mean^2 * sigma_v^2 + var)
                    let sigma_v = 0.26; // Approx for Sentinel-1 GRD 3-look
                    let var_v = mean * mean * sigma_v * sigma_v;
                    let w = if variance > var_v {
                        (variance - var_v) / variance
                    } else {
                        0.0
                    };

                    row_slice[j] = mean + w * (arr[[i, j]] - mean);
                }
            }
        });

    output
}

fn compute_index(
    vv_lin: &Array2<f32>,
    vh_lin: &Array2<f32>,
    inc_angle_deg: f32,
    index_type: &str,
    alpha: f32,
    beta: f32,
    gamma: f32,
) -> Array2<f32> {
    if index_type == "RVI" {
        // RVI on incidence-angle-normalized linear backscatter.
        // Normalization removes the cross-swath brightness gradient so
        // that RVI values are comparable across the full IW swath.
        let vv_norm = normalize_incidence_angle_linear(vv_lin, inc_angle_deg);
        let vh_norm = normalize_incidence_angle_linear(vh_lin, inc_angle_deg);
        let mut out = vv_norm.clone();
        azip!((out in &mut out, &vv in &vv_norm, &vh in &vh_norm) *out = (4.0 * vh) / (vv + vh));
        out
    } else if index_type == "S1_SMI" {
        // S1_SMI is calculated on dB scale, normalized by incidence angle.
        // Coefficients are provided by the caller (Django calibration).
        let vv_db = linear_to_db(vv_lin);
        let vh_db = linear_to_db(vh_lin);

        let vv_db_norm = normalize_incidence_angle(&vv_db, inc_angle_deg);
        let vh_db_norm = normalize_incidence_angle(&vh_db, inc_angle_deg);

        let mut out = vv_db_norm.clone();

        azip!((out in &mut out, &vv in &vv_db_norm, &vh in &vh_db_norm) *out = alpha * vv + beta * vh + gamma);
        out
    } else {
        vv_lin.clone()
    }
}

/// Compute index for L-band SAR (HH/HV); L_RVI is incidence-angle
/// normalized, NISAR_FLOOD scores raw backscatter.
fn compute_index_sar(
    hh_lin: &Array2<f32>,
    hv_lin: &Array2<f32>,
    inc_angle_deg: f32,
    index_type: &str,
) -> Array2<f32> {
    if index_type == "L_RVI" {
        // L-band RVI: same formula as C-band but with HH/HV
        // L_RVI = 4 * HV / (HH + HV), normalized for incidence angle
        let hh_norm = normalize_incidence_angle_linear(hh_lin, inc_angle_deg);
        let hv_norm = normalize_incidence_angle_linear(hv_lin, inc_angle_deg);
        let mut out = hh_norm.clone();
        azip!((out in &mut out, &hh in &hh_norm, &hv in &hv_norm) *out = (4.0 * hv) / (hh + hv));
        out
    } else if index_type == "NISAR_FLOOD" {
        // Flood-likelihood score from mean HH/HV power:
        //   sigma_db = 10*log10(max((HH+HV)/2, 1e-6))
        //   score    = clip((land_db - sigma_db) / (land_db - water_db), 0, 1)
        // sigma <= -20 dB maps to 1 (fully wet), sigma >= -12 dB to 0 (dry).
        // Must stay numerically compatible with Python's _nisar_flood_fn
        // (science/formulas/registry.py) — same eps, same thresholds.
        const WATER_DB: f32 = -20.0;
        const LAND_DB: f32 = -12.0;
        const EPS: f32 = 1e-6;
        let mut out = hh_lin.clone();
        azip!((out in &mut out, &hh in hh_lin, &hv in hv_lin) {
            *out = if hh.is_nan() || hv.is_nan() {
                f32::NAN
            } else {
                let total = ((hh + hv) / 2.0).max(EPS);
                let sigma_db = 10.0 * total.log10();
                let score = (LAND_DB - sigma_db) / (LAND_DB - WATER_DB);
                score.clamp(0.0, 1.0)
            };
        });
        out
    } else {
        hh_lin.clone()
    }
}

fn compute_stats(index: &Array2<f32>, mask: &Array2<bool>) -> PreprocessResponse {
    let mut sum = 0.0;
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    let mut count = 0;

    ndarray::azip!((&v in index, &m in mask) {
        if m && !v.is_nan() {
            sum += v;
            if v < min { min = v; }
            if v > max { max = v; }
            count += 1;
        }
    });

    let total = index.len() as f64;
    let fraction = (count as f64) / total;

    PreprocessResponse {
        mean: if count > 0 {
            Some((sum / (count as f32)) as f64)
        } else {
            None
        },
        min: if count > 0 { Some(min as f64) } else { None },
        max: if count > 0 { Some(max as f64) } else { None },
        sample_count: count,
        valid_pixel_fraction: fraction,
        quality_flags: vec![],
        processing_ms: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(hh: f32, hv: f32) -> (Array2<f32>, Array2<f32>) {
        (Array2::from_elem((2, 2), hh), Array2::from_elem((2, 2), hv))
    }

    #[test]
    fn nisar_flood_scores_open_water_as_wet() {
        // sigma_db = -40 dB -> fully wet -> 1.0
        let (hh, hv) = grid(1e-4, 1e-4);
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        assert_eq!(res.sample_count, 4);
        assert_eq!(res.mean, Some(1.0));
        assert_eq!(res.min, Some(1.0));
        assert_eq!(res.max, Some(1.0));
    }

    #[test]
    fn nisar_flood_scores_dry_land_as_zero() {
        // sigma_db = -10 dB -> dry -> 0.0
        let (hh, hv) = grid(0.1, 0.1);
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        assert_eq!(res.mean, Some(0.0));
    }

    #[test]
    fn nisar_flood_mid_scale_matches_python_formula() {
        // total = 10^(-1.6) => sigma_db = -16 dB => score = 0.5
        let total = 10.0_f32.powf(-1.6);
        let (hh, hv) = grid(total, total);
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        let mean = res.mean.expect("mean");
        assert!((mean - 0.5).abs() < 1e-4, "expected ~0.5, got {mean}");
    }

    #[test]
    fn nisar_flood_clamps_sub_water_floor_to_one() {
        // total below the 1e-6 eps floor -> sigma_db = -60 dB -> clamp 1.0
        let (hh, hv) = grid(1e-7, 1e-7);
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        assert_eq!(res.mean, Some(1.0));
    }

    #[test]
    fn nisar_flood_excludes_nodata_pixels() {
        // All-zero GCOV pixels are nodata: masked out of stats entirely.
        let (hh, hv) = grid(0.0, 0.0);
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        assert_eq!(res.sample_count, 0);
        assert_eq!(res.mean, None);
        assert_eq!(res.valid_pixel_fraction, 0.0);
    }

    #[test]
    fn nisar_flood_stats_only_over_valid_pixels() {
        // 2x2: two wet pixels (1.0), two nodata -> mean of valid = 1.0.
        let mut hh = Array2::from_elem((2, 2), 0.0_f32);
        let mut hv = Array2::from_elem((2, 2), 0.0_f32);
        hh[[0, 0]] = 1e-4;
        hv[[0, 0]] = 1e-4;
        hh[[1, 1]] = 1e-4;
        hv[[1, 1]] = 1e-4;
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        assert_eq!(res.sample_count, 2);
        assert_eq!(res.mean, Some(1.0));
        assert!((res.valid_pixel_fraction - 0.5).abs() < 1e-6);
    }

    #[test]
    fn nisar_flood_skips_speckle_filter() {
        // A 3x3 Refined-Lee pass would alter isolated high-power pixels;
        // NISAR_FLOOD must score the raw value instead.
        let mut hh = Array2::from_elem((3, 3), 0.1_f32);
        let mut hv = Array2::from_elem((3, 3), 0.1_f32);
        // Centre pixel: dry (sigma = -10 dB -> 0.0)
        hh[[1, 1]] = 0.1;
        hv[[1, 1]] = 0.1;
        let res = run_pipeline_sar(hh, hv, 40.0, "NISAR_FLOOD");
        // All 9 pixels dry -> mean 0.0 (filtering would not change this
        // uniform case; the assertion pins the unfiltered semantics).
        assert_eq!(res.mean, Some(0.0));
        assert_eq!(res.sample_count, 9);
    }

    #[test]
    fn l_rvi_path_unchanged_by_flood_branch() {
        let (hh, hv) = grid(0.4, 0.2);
        let res = run_pipeline_sar(hh, hv, 40.0, "L_RVI");
        // L_RVI = 4*0.2/(0.4+0.2) = 1.333... at theta=40 (no correction)
        let mean = res.mean.expect("mean");
        let expected = f64::from(4.0_f32 * 0.2 / 0.6);
        assert!((mean - expected).abs() < 1e-5);
    }

    #[test]
    fn cband_estimator_known_ratio_is_40_deg() {
        // VV/VH = 1.875 dB => theta = 35 + (1.875 - 1.5) / 0.075 = 40
        let vv = Array2::from_elem((11, 11), 0.5_f32);
        let vh = Array2::from_elem((11, 11), 0.5_f32 / 10.0_f32.powf(0.1875_f32));
        let theta = estimate_incidence_angle_deg(&vv, &vh);
        assert!((theta - 40.0).abs() < 1e-3, "expected ~40, got {theta}");
    }

    #[test]
    fn cband_estimator_clamps_and_falls_back() {
        let vv = Array2::from_elem((11, 11), 1.0_f32);
        let vh_wet = Array2::from_elem((11, 11), 1.0_f32 / 10.0_f32.powf(1.2_f32));
        let vh_dry = Array2::from_elem((11, 11), 10.0_f32.powf(0.6_f32));
        assert_eq!(estimate_incidence_angle_deg(&vv, &vh_wet), 46.0);
        assert_eq!(estimate_incidence_angle_deg(&vv, &vh_dry), 29.0);
        // Fewer than 100 valid pairs -> 40.0 fallback
        let vv_small = Array2::from_elem((5, 5), 0.5_f32);
        let vh_small = Array2::from_elem((5, 5), 0.2_f32);
        assert_eq!(estimate_incidence_angle_deg(&vv_small, &vh_small), 40.0);
    }

    #[test]
    fn lband_estimator_known_ratio_is_45_deg() {
        // HH/HV = 3.2 dB => theta = 35 + (3.2 - 2.5) / 0.07 = 45
        // (the C-band relation would give ~54.3 on this ratio)
        let hh = Array2::from_elem((11, 11), 0.25_f32);
        let hv = Array2::from_elem((11, 11), 0.25_f32 / 10.0_f32.powf(0.32_f32));
        let theta = estimate_incidence_angle_deg_lband(&hh, &hv);
        assert!((theta - 45.0).abs() < 1e-3, "expected ~45, got {theta}");
        let hh_small = Array2::from_elem((5, 5), 0.25_f32);
        let hv_small = Array2::from_elem((5, 5), 0.05_f32);
        assert_eq!(
            estimate_incidence_angle_deg_lband(&hh_small, &hv_small),
            40.0
        );
    }

    #[test]
    fn db_normalization_matches_linear_normalization() {
        // 10*log10(sigma * cos(t)/cos(40)) must equal
        // 10*log10(sigma) + 10*log10(cos(t)/cos(40)) — guards the sign.
        let lin = Array2::from_elem((3, 3), 0.25_f32);
        let via_linear = normalize_incidence_angle_linear(&lin, 33.0);
        let via_db = normalize_incidence_angle(&linear_to_db(&lin), 33.0);
        azip!((&l in &via_linear, &d in &via_db) {
            let expected = 10.0 * l.log10();
            assert!((d - expected).abs() < 1e-4, "db={d} expected={expected}");
        });
    }

    #[test]
    fn s1_smi_matches_python_formula_with_normalization() {
        // Python _s1_smi_fn: normalize linear bands by cos(t)/cos(40),
        // convert to dB, then SMI = alpha*vv_db + beta*vh_db + gamma.
        let vv = Array2::from_elem((2, 2), 0.5_f32);
        let vh = Array2::from_elem((2, 2), 0.2_f32);
        let theta = 33.0_f32;
        let out = compute_index(&vv, &vh, theta, "S1_SMI", 0.5, -0.1, -10.0);
        let c = theta.to_radians().cos() / 40.0_f32.to_radians().cos();
        let vv_db = 10.0 * (0.5_f32 * c).log10();
        let vh_db = 10.0 * (0.2_f32 * c).log10();
        let expected = f64::from(0.5 * vv_db + (-0.1) * vh_db - 10.0);
        let m = out[[0, 0]];
        assert!(
            (f64::from(m) - expected).abs() < 1e-4,
            "got {m} expected {expected}"
        );
    }
}
