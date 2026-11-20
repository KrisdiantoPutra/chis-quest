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

const BG: Color32 = Color32::from_rgb(0x08, 0x09, 0x0a);
const CARD: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0b);
const ROW: Color32 = Color32::from_rgb(0x15, 0x15, 0x17);
const BORDER_DIM: Color32 = Color32::from_rgb(60, 60, 66);
const TEXT: Color32 = Color32::from_rgb(230, 230, 234);
const DIM: Color32 = Color32::from_rgb(110, 110, 118);
const GREEN: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
const RED: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);

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

        // 1. Coba ambil MachineGuid dari Registry (ID unik instalasi Windows)
        if let Ok(output) = Command::new("reg")
            .args(["query", "HKLM\\SOFTWARE\\Microsoft\\Cryptography", "/v", "MachineGuid"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("MachineGuid") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let guid = parts[2].trim();
                        if !guid.is_empty() {
                            return format!("WIN-{}", guid);
                        }
                    }
                }
            }
        }

        // 2. Coba ambil Serial Number Partisi Drive C:
        if let Ok(output) = Command::new("cmd")
            .args(["/c", "vol C:"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("Serial Number is") {
                    let parts: Vec<&str> = line.split("Serial Number is").collect();
                    if parts.len() > 1 {
                        let vol = parts[1].trim();
                        if !vol.is_empty() {
                            return format!("VOL-{}", vol);
                        }
                    }
                }
            }
        }

        // 3. Coba wmic csproduct get uuid
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

    #[cfg(target_os = "linux")]
    {
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(content) = std::fs::read_to_string(path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return format!("LNX-{}", trimmed);
                }
            }
        }
    }

    // 4. Fallback unik berbasis username komputer + hostname komputer
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "USER".to_string());
    let comp = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "PC".to_string());
    format!("DEV-{}-{}", comp, user)
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

        ui.vertical(|ui| {
            // Header matching chis-quest
            ui.horizontal(|ui| {
                if let Some(t) = logo {
                    ui.image((t.id(), egui::vec2(38.0, 38.0)));
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.heading(egui::RichText::new("chis-quest").strong());
                    ui.label(egui::RichText::new("license activation").small().color(DIM));
                });
            });

            ui.add_space(16.0);

            // Card container matching chis-quest card_frame
            egui::Frame::none()
                .fill(CARD)
                .stroke(Stroke::new(1.5, BORDER_DIM))
                .rounding(Rounding::same(16.0))
                .inner_margin(Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    // Card Title
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("CHIS AUTH")
                                .strong()
                                .size(14.0)
                                .color(GREEN),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("v2.0").small().color(DIM));
                        });
                    });

                    ui.add_space(10.0);

                    // HWID in row_frame style
                    egui::Frame::none()
                        .fill(ROW)
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("HWID").small().color(DIM));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let short = if self.hwid.len() > 20 {
                                            format!("{}...", &self.hwid[..20])
                                        } else {
                                            self.hwid.clone()
                                        };
                                        ui.label(
                                            egui::RichText::new(short)
                                                .monospace()
                                                .small()
                                                .color(TEXT),
                                        );
                                    },
                                );
                            });
                        });

                    ui.add_space(12.0);

                    // License Key label
                    ui.label(egui::RichText::new("License Key").size(13.0).color(TEXT));

                    ui.add_space(4.0);

                    // TextEdit matching search input
                    let text_edit = egui::TextEdit::singleline(&mut self.key_input)
                        .hint_text("CHIS-XXXX-XXXX-XXXX")
                        .desired_width(f32::INFINITY)
                        .margin(egui::vec2(10.0, 8.0));
                    ui.add(text_edit);

                    ui.add_space(8.0);

                    // Remember Key checkbox
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.remember, "");
                        ui.label(
                            egui::RichText::new("Remember key on this device")
                                .small()
                                .color(DIM),
                        );
                    });

                    // Status Message
                    if !self.status_msg.is_empty() {
                        ui.add_space(8.0);
                        let color = if self.is_error { RED } else { GREEN };
                        ui.label(egui::RichText::new(&self.status_msg).small().color(color));
                    }

                    ui.add_space(14.0);

                    // Action Button styled matching chis-quest
                    let btn_text = if self.is_loading {
                        "Authenticating..."
                    } else {
                        "Activate & Launch"
                    };

                    let btn = egui::Button::new(
                        egui::RichText::new(btn_text)
                            .strong()
                            .size(13.5)
                            .color(Color32::WHITE),
                    )
                    .fill(if self.is_loading {
                        Color32::from_rgb(35, 35, 40)
                    } else {
                        Color32::from_rgb(35, 134, 54)
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if self.is_loading {
                            BORDER_DIM
                        } else {
                            GREEN
                        },
                    ))
                    .rounding(Rounding::same(8.0))
                    .min_size(Vec2::new(ui.available_width(), 36.0));

                    if ui.add_enabled(!self.is_loading, btn).clicked() {
                        self.trigger_validation(false);
                    }
                });

            ui.add_space(16.0);

            // Subtle note at bottom
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Internet connection required to activate")
                        .small()
                        .color(DIM),
                );
            });
        });
    }
}
