//! eu5-rnw 的產生器核心：讀取遊戲檔、生成新世界。視窗（`ui/`）只呼叫這裡。

pub mod assign;
pub mod domain;
pub mod error;
pub mod game;
pub mod generate;
pub mod geography;
pub mod islands;
pub mod land;
pub mod locations;
pub mod noise;
pub mod partition;
pub mod pdx;
pub mod rng;
pub mod validate;
pub mod vdf;
