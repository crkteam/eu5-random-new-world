//! 可重現的亂數：SplitMix64（與原專案 `rnw.rng` 相同的演算法）。
//!
//! 同一個種子永遠產生同一串數字。第 i 個數可以直接用 `at(seed, i)` 算出，不必先算前面的，
//! 所以平行計算時每個執行緒各算自己的索引，結果與執行緒數無關。

/// SplitMix64 每一步加的常數（黃金比例）。
pub const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64 的輸出函式：把一個 64 位元數打散。
pub fn mix64(value: u64) -> u64 {
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 種子 `seed` 的第 `index` 個數（從 0 起），等於 `Rng::new(seed)` 的第 `index + 1` 次 `next_u64`。
pub fn at(seed: u64, index: u64) -> u64 {
    mix64(seed.wrapping_add(index.wrapping_add(1).wrapping_mul(GAMMA)))
}

/// 整數格點 (x, y) 的雜湊；噪聲用它決定每個格點的值，不需要依序取數。
pub fn hash_point(key: u64, x: i64, y: i64) -> u64 {
    let z = key ^ (x as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ (y as u64).wrapping_mul(0xA076_1D64_78BD_642F);
    mix64(z.wrapping_add(GAMMA))
}

/// 依序取數的亂數產生器。
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        mix64(self.state)
    }

    /// [0, 1) 的浮點數，取高 53 位元。
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// 用 `threads` 個執行緒算出種子的前 `count` 個數，回傳全部的和（wrapping）當作檢查碼。
/// 用來證明結果與執行緒數無關。
pub fn checksum(seed: u64, count: u64, threads: u64) -> u64 {
    let threads = threads.max(1);
    let chunk = count.div_ceil(threads);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|t| {
                scope.spawn(move || {
                    let (start, end) = (t * chunk, ((t + 1) * chunk).min(count));
                    (start..end).fold(0u64, |sum, i| sum.wrapping_add(at(seed, i)))
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("checksum worker panicked"))
            .fold(0u64, u64::wrapping_add)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Java `SplittableRandom` 與 SplitMix64 參考實作公開的種子 1234567 數列。
    #[test]
    fn matches_the_reference_sequence() {
        let mut rng = Rng::new(1_234_567);
        let first: Vec<u64> = (0..5).map(|_| rng.next_u64()).collect();
        assert_eq!(
            first,
            [
                6_457_827_717_110_365_317,
                3_203_168_211_198_807_973,
                9_817_491_932_198_370_423,
                4_593_380_528_125_082_431,
                16_408_922_859_458_223_821,
            ]
        );
    }

    #[test]
    fn same_seed_same_numbers() {
        let a: Vec<u64> = { let mut r = Rng::new(319_218_363); (0..1000).map(|_| r.next_u64()).collect() };
        let b: Vec<u64> = { let mut r = Rng::new(319_218_363); (0..1000).map(|_| r.next_u64()).collect() };
        assert_eq!(a, b);
        let c: Vec<u64> = { let mut r = Rng::new(319_218_364); (0..1000).map(|_| r.next_u64()).collect() };
        assert_ne!(a, c);
    }

    #[test]
    fn at_matches_the_sequence() {
        let mut rng = Rng::new(42);
        for index in 0..1000 {
            assert_eq!(at(42, index), rng.next_u64());
        }
    }

    /// 與原專案 `rnw.rng.hash_point` 算出的值相同。
    #[test]
    fn hash_point_matches_the_original() {
        assert_eq!(hash_point(0, 0, 0), mix64(GAMMA));
        assert_eq!(mix64(GAMMA), 0xe220_a839_7b1d_cdaf);
        assert_eq!(hash_point(12345, -7, 99), 0x3faa_e2b1_ae01_ae3b);
        assert_eq!(hash_point(u64::MAX, 1 << 40, -(1 << 40)), 0x3c6d_37e1_b025_660e);
    }

    #[test]
    fn floats_stay_in_range() {
        let mut rng = Rng::new(7);
        assert!((0..10_000).map(|_| rng.next_f64()).all(|x| (0.0..1.0).contains(&x)));
    }

    #[test]
    fn threads_do_not_change_the_result() {
        let one = checksum(319_218_363, 100_003, 1);
        for threads in [2, 3, 8, 16] {
            assert_eq!(checksum(319_218_363, 100_003, threads), one);
        }
    }
}
