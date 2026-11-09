//! eu5-rnw 的視窗（egui）。
//!
//! 版型：上方標題列、左側設定面板、中央地圖區、下方狀態列。
//! 目前只有畫面骨架；「生成」之後會呼叫 `core/` 的產生器。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

/// 原專案的世界類型（之後由 core 提供）。
const PRESETS: [(&str, &str); 6] = [
    ("continents", "大陸"),
    ("supercontinent", "超大陸"),
    ("small_continents", "小型大陸"),
    ("archipelago", "群島"),
    ("islands", "島嶼"),
    ("wall", "高牆"),
];

struct App {
    preset: usize,
    seed: u64,
    status: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let status = match load_cjk_font(&cc.egui_ctx) {
            Ok(()) => "就緒".to_owned(),
            Err(message) => message,
        };
        Self { preset: 0, seed: 319_218_363, status }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("設定");
        ui.add_space(8.0);

        ui.label("世界類型");
        egui::ComboBox::from_id_salt("preset")
            .selected_text(PRESETS[self.preset].1)
            .show_ui(ui, |ui| {
                for (index, (_, name)) in PRESETS.iter().enumerate() {
                    ui.selectable_value(&mut self.preset, index, *name);
                }
            });
        ui.add_space(8.0);

        ui.label("種子");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.seed));
            if ui.button("隨機").clicked() {
                self.seed = random_seed();
            }
        });
        ui.add_space(16.0);

        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new("生成")).clicked() {
            let (key, _) = PRESETS[self.preset];
            self.status = format!("尚未實作：將以 {key}、種子 {} 生成", self.seed);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("title").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("eu5-rnw");
                ui.label("EU5 隨機新世界產生器（Rust 重寫版）");
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        egui::Panel::left("settings")
            .resizable(false)
            .exact_size(240.0)
            .show(ui, |ui| self.settings(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.weak("地圖預覽（生成後顯示）");
            });
        });
    }
}

/// egui 內建字型沒有中文字，從 Windows 載入微軟正黑體。
fn load_cjk_font(ctx: &egui::Context) -> Result<(), String> {
    let path = std::path::Path::new(r"C:\Windows\Fonts\msjh.ttc");
    let bytes = std::fs::read(path)
        .map_err(|e| format!("無法載入中文字型 {}：{e}", path.display()))?;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    Ok(())
}

/// 用時間當亂數來源；種子本身才是可重現的關鍵。
fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (nanos as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 1
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("eu5-rnw")
            .with_inner_size([1160.0, 740.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("eu5-rnw", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
