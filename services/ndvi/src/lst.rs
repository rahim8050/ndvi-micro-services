//! LST zonal statistics pipeline.
//!
//! Python engines download and open LST products (NetCDF / COG), then
//! POST the extracted temperature grid here. This module computes
//! mean / min / max over finite pixels (optionally restricted to a
//! farm-polygon mask), mirroring the Python `nanmean` fallback.

use ndarray::Array2;

use crate::models::{LstRequest, LstResponse};

/// Compute LST zonal stats for a flat temperature grid.
///
/// Valid pixels are those that are present (`Some` and finite) and,
/// when a mask is provided, marked `true` (inside the farm). Returns
/// an error when dimensions do not match `width * height`.
pub fn run_pipeline_lst(req: &LstRequest) -> Result<LstResponse, String> {
    let expected = req.width * req.height;
    if req.values.len() != expected {
        return Err(format!(
            "values: expected {} elements ({}x{}), got {}",
            expected,
            req.width,
            req.height,
            req.values.len()
        ));
    }

    let geometry_mask = match &req.mask {
        Some(vals) => {
            if vals.len() != expected {
                return Err(format!(
                    "mask: expected {} elements, got {}",
                    expected,
                    vals.len()
                ));
            }
            Array2::from_shape_vec((req.height, req.width), vals.clone())
                .map_err(|e| format!("mask: shape error: {}", e))?
        }
        None => Array2::from_elem((req.height, req.width), true),
    };

    // JSON null → None → treated as invalid (same as NaN).
    let values = Array2::from_shape_vec(
        (req.height, req.width),
        req.values
            .iter()
            .map(|v| v.unwrap_or(f32::NAN))
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("values: shape error: {}", e))?;

    let (sum, min, max, count) = compute_lst_stats(&values, &geometry_mask);

    Ok(LstResponse {
        mean: if count > 0 {
            Some(sum / count as f64)
        } else {
            None
        },
        min: if count > 0 { Some(min as f64) } else { None },
        max: if count > 0 { Some(max as f64) } else { None },
        sample_count: count,
        processing_ms: 0.0,
    })
}

/// Sum / min / max / count of finite pixels inside the mask.
fn compute_lst_stats(values: &Array2<f32>, mask: &Array2<bool>) -> (f64, f32, f32, u64) {
    let mut sum = 0.0_f64;
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    let mut count = 0_u64;

    ndarray::azip!((&v in values, &m in mask) {
        if m && v.is_finite() {
            sum += v as f64;
            if v < min { min = v; }
            if v > max { max = v; }
            count += 1;
        }
    });

    (sum, min, max, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request(values: Vec<Option<f32>>, width: usize, height: usize) -> LstRequest {
        LstRequest {
            values,
            width,
            height,
            mask: None,
        }
    }

    #[test]
    fn test_basic_stats() {
        // 2x2 Kelvin grid
        let req = base_request(
            vec![Some(300.0), Some(302.0), Some(298.0), Some(304.0)],
            2,
            2,
        );
        let res = run_pipeline_lst(&req).unwrap();
        assert_eq!(res.sample_count, 4);
        let mean = res.mean.unwrap();
        assert!((mean - 301.0).abs() < 1e-5, "mean={}", mean);
        assert_eq!(res.min, Some(298.0));
        assert_eq!(res.max, Some(304.0));
    }

    #[test]
    fn test_nan_excluded() {
        let req = base_request(vec![Some(300.0), None, Some(302.0), None], 2, 2);
        let res = run_pipeline_lst(&req).unwrap();
        assert_eq!(res.sample_count, 2);
        let mean = res.mean.unwrap();
        assert!((mean - 301.0).abs() < 1e-5, "mean={}", mean);
        assert_eq!(res.min, Some(300.0));
        assert_eq!(res.max, Some(302.0));
    }

    #[test]
    fn test_geometry_mask_excludes_outside() {
        let mut req = base_request(
            vec![Some(300.0), Some(302.0), Some(304.0), Some(306.0)],
            2,
            2,
        );
        req.mask = Some(vec![true, false, true, false]);
        let res = run_pipeline_lst(&req).unwrap();
        assert_eq!(res.sample_count, 2);
        let mean = res.mean.unwrap();
        assert!((mean - 302.0).abs() < 1e-5, "mean={}", mean);
        assert_eq!(res.min, Some(300.0));
        assert_eq!(res.max, Some(304.0));
    }

    #[test]
    fn test_all_nan_returns_none_stats() {
        let req = base_request(vec![None, None], 2, 1);
        let res = run_pipeline_lst(&req).unwrap();
        assert_eq!(res.sample_count, 0);
        assert!(res.mean.is_none());
        assert!(res.min.is_none());
        assert!(res.max.is_none());
    }

    #[test]
    fn test_dimension_mismatch_errors() {
        let req = base_request(vec![Some(300.0), Some(302.0)], 2, 2);
        let res = run_pipeline_lst(&req);
        assert!(res.is_err());
    }

    #[test]
    fn test_mask_length_mismatch_errors() {
        let mut req = base_request(
            vec![Some(300.0), Some(302.0), Some(304.0), Some(306.0)],
            2,
            2,
        );
        req.mask = Some(vec![true, false]);
        let res = run_pipeline_lst(&req);
        assert!(res.is_err());
    }

    #[test]
    fn test_infinity_excluded() {
        let req = base_request(
            vec![
                Some(300.0),
                Some(f32::INFINITY),
                Some(302.0),
                Some(f32::NEG_INFINITY),
            ],
            2,
            2,
        );
        let res = run_pipeline_lst(&req).unwrap();
        assert_eq!(res.sample_count, 2);
    }
}
