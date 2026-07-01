use ndarray::{azip, Array2};
use rayon::prelude::*;
use crate::models::PreprocessResponse;

pub fn run_pipeline(
    vv_raw: Array2<f32>,
    vh_raw: Array2<f32>,
    inc_angle_deg: f32,
    index_type: &str,
) -> PreprocessResponse {
    let (vv, mask_vv) = mask_nodata(&vv_raw);
    let (vh, mask_vh) = mask_nodata(&vh_raw);
    
    // Pixel-wise AND of masks
    let mut valid_mask = mask_vv.clone();
    azip!((v in &mut valid_mask, &vh in &mask_vh) *v = *v && vh);

    // Apply speckle filter on linear backscatter
    let vv_filtered = refined_lee_filter(&vv, 3);
    let vh_filtered = refined_lee_filter(&vh, 3);

    // Compute index (handles dB conversion internally if needed)
    let index = compute_index(&vv_filtered, &vh_filtered, inc_angle_deg, index_type);

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

fn normalize_incidence_angle(db_arr: &Array2<f32>, theta_local_deg: f32) -> Array2<f32> {
    let theta_ref_rad = 40.0_f32.to_radians();
    let theta_local_rad = theta_local_deg.to_radians();
    let correction = theta_ref_rad.cos() / theta_local_rad.cos();

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
                    let w = if variance > var_v { (variance - var_v) / variance } else { 0.0 };
                    
                    row_slice[j] = mean + w * (arr[[i, j]] - mean);
                }
            }
        });

    output
}

fn compute_index(vv_lin: &Array2<f32>, vh_lin: &Array2<f32>, inc_angle_deg: f32, index_type: &str) -> Array2<f32> {
    if index_type == "RVI" {
        // RVI is calculated on linear scale
        let mut out = vv_lin.clone();
        azip!((out in &mut out, &vv in vv_lin, &vh in vh_lin) *out = (4.0 * vh) / (vv + vh));
        out
    } else if index_type == "S1_SMI" {
        // S1_SMI is calculated on dB scale, normalized by incidence angle
        let vv_db = linear_to_db(vv_lin);
        let vh_db = linear_to_db(vh_lin);
        
        let vv_db_norm = normalize_incidence_angle(&vv_db, inc_angle_deg);
        let vh_db_norm = normalize_incidence_angle(&vh_db, inc_angle_deg);
        
        let mut out = vv_db_norm.clone();
        let alpha = 0.70;
        let beta = -0.30;
        let gamma = 0.50;
        
        azip!((out in &mut out, &vv in &vv_db_norm, &vh in &vh_db_norm) *out = alpha * vv + beta * vh + gamma);
        out
    } else {
        vv_lin.clone()
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
        mean: if count > 0 { Some((sum / (count as f32)) as f64) } else { None },
        min: if count > 0 { Some(min as f64) } else { None },
        max: if count > 0 { Some(max as f64) } else { None },
        sample_count: count,
        valid_pixel_fraction: fraction,
        quality_flags: vec![],
        processing_ms: 0.0,
    }
}

