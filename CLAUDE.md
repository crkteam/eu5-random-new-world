# eu5-rnw 工作規則

這個倉庫是 [Random New World](https://github.com/Sir-Starch/eu5-random-new-world)（MPL-2.0）的
個人改寫版：用 Rust 重寫 EU5 的新世界地圖產生器，給擁有者自己玩。

## 結構

根目錄是新專案 eu5-rnw 的 Cargo workspace：

- `core/`：產生器（讀遊戲檔、生成地圖）。視窗只呼叫這裡。
- `ui/`：egui 視窗（eframe 0.35；API 用 `App::ui`、`egui::Panel`）。
- 編譯結果：`target/release/eu5-rnw.exe`；`cargo run --release` 開啟視窗。
- `old/`：原作者的完整專案，**只當參考**，已排除在 workspace 外。它自己的 `old/AGENTS.md`
  只規範 `old/` 內的工作，不適用於新專案。不要修改 `old/`，除非擁有者要求。

## 工作方式

- 用繁體中文回覆。一次只做一步，做完驗證、回報，再等擁有者決定下一步。
- 目前目標：在視窗中生成新世界的地圖預覽（8 步計畫，見對話與提交紀錄）。
- 原專案的作法可以參考，但不必照搬它的版本／epoch／golden 制度。

## 規則

- 遊戲安裝資料夾只讀，永遠不修改。不要啟動或操作 EU5；遊戲內測試由擁有者自己做。
- 錯了就明確失敗並說明原因，不做靜默的備援或事後修補。
- 生成結果要可重現：同樣的種子和設定 → 同樣的輸出，與執行緒數無關。
- 遊戲衍生資料（生成的 mod、地圖點陣）只留在本機，不進 git。
- 檢查：在根目錄執行 `cargo test --workspace` 與
  `cargo clippy --workspace --all-targets --release -- -D warnings`。
  需要已安裝遊戲的測試標 `#[ignore]`，用 `cargo test -p eu5-rnw-core --release -- --ignored --nocapture` 執行。
- 提交：只在擁有者要求時提交；訊息用英文，說明做了什麼與為什麼。
