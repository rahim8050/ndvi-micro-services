use std::collections::HashMap;

use ndarray::Array2;

use crate::models::{SpectralRequest, SpectralResponse};

/// Run the optical/spectral compute pipeline.
///
/// Given named band arrays, a target index type, and an optional cloud mask,
/// compute the spectral index pixel-by-pixel and return tile-level statistics.
pub fn run_pipeline_spectral(req: &SpectralRequest) -> Result<SpectralResponse, String> {
    let expected = req.width * req.height;

    let nodata = req.nodata.unwrap_or(0.0);

    // Build per-band arrays, validating dimensions.
    let mut bands: HashMap<String, Array2<f32>> = HashMap::new();
    for (name, values) in &req.bands {
        if values.len() != expected {
            return Err(format!(
                "band '{}': expected {} elements ({}x{}), got {}",
                name,
                expected,
                req.width,
                req.height,
                values.len()
            ));
        }
        let arr = Array2::from_shape_vec((req.height, req.width), values.clone())
            .map_err(|e| format!("band '{}': shape error: {}", name, e))?;
        bands.insert(name.clone(), arr);
    }

    // Build cloud mask (true = cloudy / invalid).
    let cloud_mask = match &req.cloud_mask {
        Some(vals) => {
            if vals.len() != expected {
                return Err(format!(
                    "cloud_mask: expected {} elements, got {}",
                    expected,
                    vals.len()
                ));
            }
            Array2::from_shape_vec((req.height, req.width), vals.clone())
                .map_err(|e| format!("cloud_mask: shape error: {}", e))?
                .mapv(|v| v > 0.0)
        }
        None => Array2::from_elem((req.height, req.width), false),
    };

    // Build nodata mask: pixel is nodata if ANY band value <= nodata threshold.
    let nodata_mask = {
        let mut mask = Array2::from_elem((req.height, req.width), false);
        for band_arr in bands.values() {
            ndarray::azip!((m in &mut mask, &v in band_arr) {
                if v <= nodata { *m = true; }
            });
        }
        mask
    };

    // Combined valid mask: not nodata AND not cloud.
    let mut valid_mask = cloud_mask.mapv(|c| !c);
    ndarray::azip!((v in &mut valid_mask, &n in &nodata_mask) *v = *v && !n);

    // Compute index.
    let index = compute_spectral_index(&bands, &req.index_type)?;

    // Compute statistics.
    let total_pixels = expected as f64;
    let (sum, min, max, count) = compute_stats_par(&index, &valid_mask);
    let cloud_count = cloud_mask.iter().filter(|&&v| v).count() as f64;
    let cloud_fraction = cloud_count / total_pixels;
    let valid_pixel_fraction = (count as f64) / total_pixels;

    Ok(SpectralResponse {
        mean: if count > 0 {
            Some(sum / count as f64)
        } else {
            None
        },
        min: if count > 0 { Some(min as f64) } else { None },
        max: if count > 0 { Some(max as f64) } else { None },
        sample_count: count,
        cloud_fraction,
        valid_pixel_fraction,
        processing_ms: 0.0,
    })
}

/// Dispatch to the correct per-index formula.
fn compute_spectral_index(
    bands: &HashMap<String, Array2<f32>>,
    index_type: &str,
) -> Result<Array2<f32>, String> {
    match index_type {
        "NDVI" => Ok(ndvi(bands)),
        "NDWI" => Ok(ndwi(bands)),
        "NDMI" => Ok(ndmi(bands)),
        "NDRE" => Ok(ndre(bands)),
        "EVI" => Ok(evi(bands)),
        "IRON_OXIDE" => Ok(iron_oxide(bands)),
        "BIOMASS" => Ok(biomass(bands)),
        _ => Err(format!("unsupported spectral index: {}", index_type)),
    }
}

/// NDVI = (B08 − B04) / (B08 + B04)
/// Uses Sentinel-2 bands: B08 (NIR), B04 (Red).
fn ndvi(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let nir = &bands["B08"];
    let red = &bands["B04"];
    let mut out = nir.clone();
    ndarray::azip!((out in &mut out, &n in nir, &r in red) {
        let denom = n + r;
        *out = if denom.abs() > 1e-10 { (n - r) / denom } else { f32::NAN };
    });
    out
}

/// NDWI = (B03 − B08) / (B03 + B08)
/// Uses Sentinel-2 bands: B03 (Green), B08 (NIR).
fn ndwi(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let green = &bands["B03"];
    let nir = &bands["B08"];
    let mut out = green.clone();
    ndarray::azip!((out in &mut out, &g in green, &n in nir) {
        let denom = g + n;
        *out = if denom.abs() > 1e-10 { (g - n) / denom } else { f32::NAN };
    });
    out
}

/// NDMI = (B08 − B11) / (B08 + B11)
/// Uses Sentinel-2 bands: B08 (NIR), B11 (SWIR1).
fn ndmi(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let nir = &bands["B08"];
    let swir = &bands["B11"];
    let mut out = nir.clone();
    ndarray::azip!((out in &mut out, &n in nir, &s in swir) {
        let denom = n + s;
        *out = if denom.abs() > 1e-10 { (n - s) / denom } else { f32::NAN };
    });
    out
}

/// NDRE = (B08 − B05) / (B08 + B05)
/// Uses Sentinel-2 bands: B08 (NIR), B05 (Red Edge).
fn ndre(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let nir = &bands["B08"];
    let rededge = &bands["B05"];
    let mut out = nir.clone();
    ndarray::azip!((out in &mut out, &n in nir, &r in rededge) {
        let denom = n + r;
        *out = if denom.abs() > 1e-10 { (n - r) / denom } else { f32::NAN };
    });
    out
}

/// EVI = 2.5 × (B08 − B04) / (B08 + 6×B04 − 7.5×B02 + 1)
/// Uses Sentinel-2 bands: B08 (NIR), B04 (Red), B02 (Blue).
fn evi(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let nir = &bands["B08"];
    let red = &bands["B04"];
    let blue = &bands["B02"];
    let mut out = nir.clone();
    ndarray::azip!((out in &mut out, &n in nir, &r in red, &b in blue) {
        let denom = n + 6.0 * r - 7.5 * b + 1.0;
        *out = if denom.abs() > 1e-10 { 2.5 * (n - r) / denom } else { f32::NAN };
    });
    out
}

/// Iron Oxide = B04 / B02
/// Uses Sentinel-2 bands: B04 (Red), B02 (Blue).
/// Highlights ferric iron oxide absorption of blue light in bare soil.
fn iron_oxide(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let red = &bands["B04"];
    let blue = &bands["B02"];
    let mut out = red.clone();
    ndarray::azip!((out in &mut out, &r in red, &b in blue) {
        *out = if b.abs() > 1e-10 { r / b } else { f32::NAN };
    });
    out
}

/// BIOMASS = 68.67 × NDVI − 10.3
/// AGB (kg/m²) derived from Sentinel-2 NDVI.
fn biomass(bands: &HashMap<String, Array2<f32>>) -> Array2<f32> {
    let nir = &bands["B08"];
    let red = &bands["B04"];
    let mut out = nir.clone();
    ndarray::azip!((out in &mut out, &n in nir, &r in red) {
        let denom = n + r;
        let ndvi = if denom.abs() > 1e-10 { (n - r) / denom } else { f32::NAN };
        *out = if ndvi.is_nan() { f32::NAN } else { 68.67 * ndvi - 10.3 };
    });
    out
}

/// Parallel stats computation: sum, min, max, count of valid pixels.
fn compute_stats_par(index: &Array2<f32>, mask: &Array2<bool>) -> (f64, f32, f32, u64) {
    let mut sum = 0.0_f64;
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    let mut count = 0_u64;

    ndarray::azip!((&v in index, &m in mask) {
        if m && !v.is_nan() {
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

    fn make_2x2(band: &str, v: [f32; 4]) -> Array2<f32> {
        Array2::from_shape_vec((2, 2), v.to_vec()).unwrap()
    }

    fn base_request(index_type: &str, bands: HashMap<String, Vec<f32>>) -> SpectralRequest {
        SpectralRequest {
            bands,
            width: 2,
            height: 2,
            index_type: index_type.to_string(),
            cloud_mask: None,
            cloud_threshold: None,
            nodata: None,
        }
    }

    #[test]
    fn test_ndvi() {
        let mut bands = HashMap::new();
        bands.insert("B08".into(), vec![0.5, 0.6, 0.3, 0.4]); // NIR
        bands.insert("B04".into(), vec![0.2, 0.2, 0.1, 0.1]); // Red
        let req = base_request("NDVI", bands);
        let res = run_pipeline_spectral(&req).unwrap();
        assert!(res.mean.is_some());
        let mean = res.mean.unwrap();
        // (0.5-0.2)/(0.5+0.2) = 0.4286, (0.6-0.2)/(0.6+0.2) = 0.5, etc.
        assert!(mean > 0.3 && mean < 0.6, "NDVI mean={}", mean);
        assert_eq!(res.sample_count, 4);
    }

    #[test]
    fn test_ndwi() {
        let mut bands = HashMap::new();
        bands.insert("B03".into(), vec![0.4, 0.5, 0.3, 0.6]);
        bands.insert("B08".into(), vec![0.3, 0.4, 0.2, 0.5]);
        let req = base_request("NDWI", bands);
        let res = run_pipeline_spectral(&req).unwrap();
        assert!(res.mean.is_some());
    }

    #[test]
    fn test_cloud_mask() {
        let mut bands = HashMap::new();
        bands.insert("B08".into(), vec![0.5, 0.6, 0.3, 0.4]);
        bands.insert("B04".into(), vec![0.2, 0.2, 0.1, 0.1]);
        let mut req = base_request("NDVI", bands);
        req.cloud_mask = Some(vec![0.0, 1.0, 0.0, 0.0]); // pixel 1 is cloudy
        let res = run_pipeline_spectral(&req).unwrap();
        assert_eq!(res.sample_count, 3); // 1 cloud pixel excluded
        assert!(res.cloud_fraction > 0.2 && res.cloud_fraction < 0.3);
    }

    #[test]
    fn test_nodata_masking() {
        let mut bands = HashMap::new();
        bands.insert("B08".into(), vec![0.5, -0.01, 0.3, 0.4]);
        bands.insert("B04".into(), vec![0.2, 0.2, 0.1, 0.1]);
        let mut req = base_request("NDVI", bands);
        req.nodata = Some(0.0);
        let res = run_pipeline_spectral(&req).unwrap();
        assert_eq!(res.sample_count, 3); // 1 nodata pixel excluded
    }

    #[test]
    fn test_unsupported_index() {
        let mut bands = HashMap::new();
        bands.insert("B08".into(), vec![0.5]);
        bands.insert("B04".into(), vec![0.2]);
        let mut req = base_request("FAKE_INDEX", bands);
        req.width = 1;
        req.height = 1;
        let res = run_pipeline_spectral(&req);
        assert!(res.is_err());
    }

    #[test]
    fn test_dimension_mismatch() {
        let mut bands = HashMap::new();
        bands.insert("B08".into(), vec![0.5, 0.6]); // 2 elements, but 2x2=4 expected
        bands.insert("B04".into(), vec![0.2, 0.2, 0.1, 0.1]);
        let req = base_request("NDVI", bands);
        let res = run_pipeline_spectral(&req);
        assert!(res.is_err());
    }
}
