use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const RAW_DIR: &str = r"D:\Games\ACC_Telemetry\raw";
const SESSIONS_DIR: &str = r"D:\Games\ACC_Telemetry\sessions";
const STOP_FILE: &str = r"D:\Games\ACC_Telemetry\acr_stop";

struct AppState { recorder: Option<Child>, status: String }

impl AppState {
    fn new() -> Self { Self { recorder: None, status: "Ready".into() } }
    fn recording(&self) -> bool { self.recorder.is_some() }
    fn exe_dir() -> PathBuf { std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from(".")) }
    fn project_dir() -> PathBuf {
        Self::exe_dir().parent().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    }
    fn ensure_recorder_config(&self) -> Result<(), String> {
        let source = Self::project_dir().join("acr_recorder.toml");
        let target = Self::exe_dir().join("acr_recorder.toml");
        if !source.exists() { return Err(format!("Recorder config not found: {}", source.display())); }
        fs::copy(&source, &target).map_err(|e| format!("Could not install recorder config: {e}"))?;
        Ok(())
    }
    fn start(&mut self) {
        if self.recording() { return; }
        if let Err(e) = self.ensure_recorder_config() { self.status = e; return; }
        let _ = fs::create_dir_all(RAW_DIR);
        let _ = fs::remove_file(STOP_FILE);
        let exe = Self::exe_dir().join("acr_recorder.exe");
        match Command::new(&exe).current_dir(Self::project_dir()).spawn() {
            Ok(child) => { self.recorder = Some(child); self.status = format!("Recording → {}", RAW_DIR); }
            Err(e) => self.status = format!("Recorder start failed: {e}"),
        }
    }
    fn stop(&mut self) {
        if !self.recording() { return; }
        match fs::write(STOP_FILE, b"stop") {
            Ok(_) => self.status = "Stopping recorder and flushing data...".into(),
            Err(e) => self.status = format!("Could not create stop file: {e}"),
        }
    }
    fn poll(&mut self) {
        if let Some(child) = self.recorder.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                self.recorder = None;
                let _ = fs::remove_file(STOP_FILE);
                self.status = "Recording stopped — session ready".into();
            }
        }
    }
    fn generate_report(&mut self) {
        if self.recording() { return; }
        let ps1 = Self::project_dir().join("make_session_report.ps1");
        if !ps1.exists() { self.status = format!("Report script not found: {}", ps1.display()); return; }
        self.status = "Generating report...".into();
        match Command::new("powershell.exe").current_dir(Self::project_dir()).args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&ps1).spawn() {
            Ok(_) => self.status = format!("Report generation started → {}", SESSIONS_DIR),
            Err(e) => self.status = format!("Report start failed: {e}"),
        }
    }
}

struct ControlApp { state: AppState }

impl eframe::App for ControlApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.poll();
        let recording = self.state.recording();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(150.0, 62.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 10.0, egui::Color32::from_rgb(24, 24, 28));
                painter.rect_stroke(rect, 10.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(210, 30, 45)), egui::StrokeKind::Outside);
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, "ACC", egui::FontId::proportional(34.0), egui::Color32::WHITE);
                ui.add_space(8.0);
                ui.heading("TELEMETRY CONTROL");
                ui.label("Assetto Corsa Competizione");
            });
            ui.add_space(16.0); ui.separator(); ui.add_space(10.0);
            ui.horizontal(|ui| { ui.label("STATUS:"); ui.colored_label(if recording { egui::Color32::from_rgb(230,60,60) } else { egui::Color32::from_rgb(90,210,120) }, if recording { "● RECORDING" } else { "● READY" }); });
            ui.add_space(6.0); ui.label(format!("RAW: {}", RAW_DIR)); ui.add_space(18.0);
            if ui.add_enabled(!recording, egui::Button::new("🔴  START RECORDING").min_size(egui::vec2(300.0,48.0))).clicked() { self.state.start(); }
            ui.add_space(7.0);
            if ui.add_enabled(recording, egui::Button::new("■  STOP RECORDING").min_size(egui::vec2(300.0,48.0))).clicked() { self.state.stop(); }
            ui.add_space(7.0);
            if ui.add_enabled(!recording, egui::Button::new("📊  GENERATE REPORT").min_size(egui::vec2(300.0,48.0))).clicked() { self.state.generate_report(); }
            ui.add_space(18.0); ui.separator(); ui.add_space(8.0); ui.label(&self.state.status);
        });
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([380.0,460.0]).with_resizable(false), ..Default::default() };
    eframe::run_native("ACC Telemetry Control", options, Box::new(|_cc| Ok(Box::new(ControlApp { state: AppState::new() }))))
}
