//! 分形噪聲高度場（fractal value noise），只在遮罩內（新世界）計算。
//!
//! 每個點的值只由種子、參數與座標決定，所以平行計算的結果與執行緒數無關。
//! EU5 地圖左右相接（`wrap_x`），新世界跨過接縫，所以噪聲在 x 方向以地圖寬度為週期。

use crate::rng::{hash_point, mix64};

/// 噪聲參數。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseParams {
    /// 第一層（最大）起伏的大約寬度，以原圖像素計。
    pub feature_px: f64,
    /// 疊幾層；每層寬度減半。
    pub octaves: u32,
    /// 每層振幅乘上的比例（0–1）；越大越破碎。
    pub persistence: f64,
}

impl Default for NoiseParams {
    fn default() -> Self {
        // shortcut: 先用固定值看效果，第 7 步依世界類型決定。
        Self { feature_px: 2048.0, octaves: 6, persistence: 0.5 }
    }
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// 單層 value noise，值域 [-1, 1]；x 方向每 `cells_x` 個格子重複一次。
fn value_noise(key: u64, cells_x: i64, x: f64, y: f64) -> f64 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (sx, sy) = (smooth(x - fx), smooth(y - fy));
    let at = |dx: i64, dy: i64| {
        let h = hash_point(key, (ix + dx).rem_euclid(cells_x), iy + dy);
        (h >> 11) as f64 * (2.0 / (1u64 << 53) as f64) - 1.0
    };
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// 原圖座標 (x, y) 的分形噪聲，值域 [-1, 1]；`period` 是 x 方向的週期（地圖寬度）。
pub fn fractal(seed: u64, params: &NoiseParams, period: f64, x: f64, y: f64) -> f64 {
    let (mut sum, mut total, mut amplitude) = (0.0, 0.0, 1.0);
    for octave in 0..params.octaves {
        let wanted = params.feature_px / f64::from(1u32 << octave);
        // 讓一圈剛好是整數個格子，左右才接得起來。
        let cells_x = (period / wanted).round().max(1.0);
        let cell = period / cells_x;
        let key = mix64(seed ^ u64::from(octave).wrapping_mul(0x9E37_79B9));
        sum += amplitude * value_noise(key, cells_x as i64, x / cell, y / cell);
        total += amplitude;
        amplitude *= params.persistence;
    }
    sum / total
}

/// 一張縮小的高度場：每 `step` 原圖像素一格。遮罩外的格子是 0。
pub struct HeightField {
    pub width: usize,
    pub height: usize,
    pub step: usize,
    pub values: Vec<f32>,
}

/// 在 `width`×`height` 的格子上算噪聲（格子 (gx, gy) 對應原圖 (gx·step, gy·step)），
/// 只算 `mask` 為真的格子；`threads` 個執行緒依列分工。
pub fn height_field(
    seed: u64,
    params: &NoiseParams,
    (width, height, step): (usize, usize, usize),
    mask: &[bool],
    threads: usize,
) -> HeightField {
    assert_eq!(mask.len(), width * height, "mask must cover the grid");
    let period = (width * step) as f64;
    let mut values = vec![0.0f32; width * height];
    let rows_per = height.div_ceil(threads.max(1)).max(1);
    std::thread::scope(|scope| {
        for (chunk, part) in values.chunks_mut(rows_per * width).enumerate() {
            scope.spawn(move || {
                for (i, value) in part.iter_mut().enumerate() {
                    let at = chunk * rows_per * width + i;
                    if mask[at] {
                        let (x, y) = ((at % width * step) as f64, (at / width * step) as f64);
                        *value = fractal(seed, params, period, x, y) as f32;
                    }
                }
            });
        }
    });
    HeightField { width, height, step, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: NoiseParams = NoiseParams { feature_px: 64.0, octaves: 5, persistence: 0.5 };

    #[test]
    fn stays_in_range_and_varies() {
        let values: Vec<f64> =
            (0..2000).map(|i| fractal(1, &P, 1024.0, (i * 7) as f64, (i * 3) as f64)).collect();
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        let (min, max) = values.iter().fold((1.0f64, -1.0f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(max - min > 0.5, "noise is too flat: {min}..{max}");
    }

    #[test]
    fn wraps_around_the_map() {
        for y in [0.0, 13.5, 400.25] {
            for x in [0.0, 5.5, 333.0] {
                let a = fractal(9, &P, 1000.0, x, y);
                let b = fractal(9, &P, 1000.0, x + 1000.0, y);
                assert!((a - b).abs() < 1e-12, "x={x} y={y}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn is_continuous() {
        let a = fractal(3, &P, 1024.0, 100.0, 100.0);
        let b = fractal(3, &P, 1024.0, 100.01, 100.0);
        assert!((a - b).abs() < 0.01);
    }

    #[test]
    fn seeds_differ() {
        assert_ne!(fractal(1, &P, 1024.0, 50.0, 50.0), fractal(2, &P, 1024.0, 50.0, 50.0));
    }

    #[test]
    fn mask_and_threads() {
        let (w, h) = (64, 32);
        let mask: Vec<bool> = (0..w * h).map(|i| i % 3 != 0).collect();
        let one = height_field(5, &P, (w, h, 4), &mask, 1);
        for threads in [2, 7, 64] {
            assert_eq!(height_field(5, &P, (w, h, 4), &mask, threads).values, one.values);
        }
        assert!(mask.iter().zip(&one.values).all(|(&m, &v)| m || v == 0.0));
        assert!(one.values.iter().any(|&v| v != 0.0));
    }
}
