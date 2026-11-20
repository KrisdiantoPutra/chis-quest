use eframe::egui::{self, Color32, Margin, Rounding, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::Duration,
};

pub const API_URL: &str = "https://system-chis.vercel.app/api";
pub const APP_ID: &str = "CHISQUEST";

const COLOR_BG: Color32 = Color32::from_rgb(0x07, 0x0b, 0x13);
const COLOR_CARD: Color32 = Color32::from_rgb(0x0d, 0x14, 0x24);
const COLOR_BORDER: Color32 = Color32::from_rgb(0x1e, 0x29, 0x3b);
const COLOR_EMERALD: Color32 = Color32::from_rgb(0x10, 0xb9, 0x81);
const COLOR_TEXT: Color32 = Color32::from_rgb(0xf8, 0xfa, 0xfc);
const COLOR_MUTED: Color32 = Color32::from_rgb(0x94, 0xa3, 0xb8);
const COLOR_RED: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);

#[derive(Serialize)]
struct AuthRequest<'a> {
    license_key: &'a str,
    app_id: &'a str,
    hwid: &'a str,
}

#[derive(Deserialize, Debug)]
pub struct AuthResponse {
    pub success: bool,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    pub data: Option<LicenseData>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LicenseData {
    pub status: Option<String>,
    pub expiry_date: Option<String>,
    pub token: Option<String>,
}

pub fn get_hwid() -> String {
    #[cfg(windows)]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("wmic")
            .args(["csproduct", "get", "uuid"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.contains("UUID") {
                    return trimmed.to_string();
                }
            }
        }
    }

    format!(
        "DEV-{}-{:?}",
        std::env::consts::ARCH,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 86400
    )
}

fn license_file_path() -> PathBuf {
    crate::config::exe_dir().join("license.key")
}

pub fn load_saved_key() -> String {
    fs::read_to_string(license_file_path())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn save_key(key: &str) {
    let _ = fs::write(license_file_path(), key.trim());
}

pub fn delete_saved_key() {
    let _ = fs::remove_file(license_file_path());
}

pub struct AuthState {
    pub key_input: String,
    pub remember: bool,
    pub is_loading: bool,
    pub status_msg: String,
    pub is_error: bool,
    pub is_authenticated: bool,
    pub license_info: Option<LicenseData>,
    pub hwid: String,
    tx: Sender<Result<AuthResponse, String>>,
    rx: Receiver<Result<AuthResponse, String>>,
}

impl AuthState {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let saved = load_saved_key();
        let remember = !saved.is_empty();
        let hwid = get_hwid();

        let mut state = Self {
            key_input: saved,
            remember,
            is_loading: false,
            status_msg: String::new(),
            is_error: false,
            is_authenticated: false,
            license_info: None,
            hwid,
            tx,
            rx,
        };

        if !state.key_input.is_empty() {
            state.trigger_validation(false);
        }

        state
    }

    pub fn trigger_validation(&mut self, is_activate: bool) {
        let key = self.key_input.trim().to_string();
        if key.is_empty() {
            self.status_msg = "Please enter your license key!".to_string();
            self.is_error = true;
            return;
        }

        self.is_loading = true;
        self.status_msg = if is_activate {
            "Activating license key on this device...".to_string()
        } else {
            "Validating license & HWID...".to_string()
        };
        self.is_error = false;

        let tx = self.tx.clone();
        let hwid = self.hwid.clone();
        let endpoint = if is_activate { "activate" } else { "validate" };

        thread::spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(12))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(format!("HTTP Client error: {e}")));
                    return;
                }
            };

            let payload = AuthRequest {
                license_key: &key,
                app_id: APP_ID,
                hwid: &hwid,
            };

            let url = format!("{API_URL}/{endpoint}");
            let res = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .json(&payload)
                .send();

            match res {
                Ok(resp) => match resp.json::<AuthResponse>() {
                    Ok(parsed) => {
                        let _ = tx.send(Ok(parsed));
                    }
                    Err(e) => {
                        let _ = tx.send(Err(format!("Failed to parse response: {e}")));
                    }
                },
                Err(e) => {
                    let _ = tx.send(Err(format!("Network connection failed: {e}")));
                }
            }
        });
    }

    pub fn check_results(&mut self) {
        if let Ok(res) = self.rx.try_recv() {
            self.is_loading = false;
            match res {
                Ok(auth_resp) => {
                    if auth_resp.success {
                        self.is_authenticated = true;
                        self.is_error = false;
                        self.status_msg = "License validated successfully! Access granted.".to_string();
                        self.license_info = auth_resp.data;

                        if self.remember {
                            save_key(&self.key_input);
                        } else {
                            delete_saved_key();
                        }
                    } else {
                        if auth_resp.code == "LICENSE_NOT_ACTIVATED" {
                            self.trigger_validation(true);
                            return;
                        }
                        self.is_authenticated = false;
                        self.is_error = true;
                        self.status_msg = format!("{} ({})", auth_resp.message, auth_resp.code);
                    }
                }
                Err(err_msg) => {
                    self.is_authenticated = false;
                    self.is_error = true;
                    self.status_msg = err_msg;
                }
            }
        }
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>) {
        self.check_results();

        let full_rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full_rect, Rounding::ZERO, COLOR_BG);

        ui.vertical_centered(|ui| {
            ui.add_space(32.0);

            if let Some(t) = logo {
                ui.image((t.id(), egui::vec2(48.0, 48.0)));
                ui.add_space(8.0);
            }

            egui::Frame::none()
                .fill(COLOR_CARD)
                .rounding(Rounding::same(12.0))
                .stroke(Stroke::new(1.0, COLOR_BORDER))
                .inner_margin(Margin::same(20.0))
                .show(ui, |ui| {
                    ui.set_max_width(290.0);

                    ui.label(
                        egui::RichText::new("CHIS AUTH")
                            .color(COLOR_EMERALD)
                            .size(19.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("License Verification")
                            .color(COLOR_MUTED)
                            .size(11.0),
                    );

                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("HWID:").color(COLOR_MUTED).size(10.0));
                        let short_hwid = if self.hwid.len() > 18 {
                            format!("{}...", &self.hwid[..18])
                        } else {
                            self.hwid.clone()
                        };
                        ui.label(
                            egui::RichText::new(short_hwid)
                                .color(Color32::from_rgb(0xcb, 0xd5, 0xe1))
                                .monospace()
                                .size(10.0),
                        );
                    });

                    ui.add_space(10.0);

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("License Key")
                                .color(COLOR_TEXT)
                                .size(11.0)
                                .strong(),
                        );
                    });

                    let text_edit = egui::TextEdit::singleline(&mut self.key_input)
                        .hint_text("CHIS-XXXX-XXXX-XXXX")
                        .desired_width(f32::INFINITY)
                        .margin(egui::vec2(8.0, 7.0));
                    ui.add(text_edit);

                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.remember, "");
                        ui.label(
                            egui::RichText::new("Remember Key")
                                .color(COLOR_MUTED)
                                .size(11.0),
                        );
                    });

                    ui.add_space(12.0);

                    let btn_text = if self.is_loading {
                        "Authenticating..."
                    } else {
                        "ACTIVATE & LAUNCH"
                    };

                    let btn = egui::Button::new(
                        egui::RichText::new(btn_text)
                            .color(Color32::WHITE)
                            .strong()
                            .size(12.0),
                    )
                    .fill(if self.is_loading { COLOR_BORDER } else { COLOR_EMERALD })
                    .rounding(Rounding::same(8.0))
                    .min_size(Vec2::new(f32::INFINITY, 34.0));

                    if ui.add_enabled(!self.is_loading, btn).clicked() {
                        self.trigger_validation(false);
                    }

                    if !self.status_msg.is_empty() {
                        ui.add_space(10.0);
                        let color = if self.is_error {
                            COLOR_RED
                        } else {
                            COLOR_EMERALD
                        };
                        ui.label(
                            egui::RichText::new(&self.status_msg)
                                .color(color)
                                .size(10.5),
                        );
                    }

                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Protected by CHIS AUTH v2.0")
                            .color(Color32::from_rgb(0x47, 0x55, 0x69))
                            .size(9.0),
                    );
                });
        });
    }
}
